//! The HTTP client settings the native adapters share: one bounded
//! connect attempt, a stall guard between stream chunks, and a name
//! lookup that cannot outlive the request.
//!
//! reqwest's stock resolver runs `getaddrinfo` on a blocking thread.
//! Nothing cancels that thread: not the request's timeout, which drops
//! the future and leaves the lookup running, and not the runtime's
//! shutdown, which waits for every blocking task. A host the resolver
//! never answers (an unreachable LAN name, a dead VPN DNS server)
//! therefore holds both the request and the process exit for the
//! resolver's own timeout. [`client_builder`] installs a resolver that
//! gives the lookup the same budget as the connect, so the request
//! fails instead of hanging.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// One connect attempt's budget.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The silence allowed between stream chunks, never the total
/// generation time.
pub const READ_TIMEOUT: Duration = Duration::from_secs(90);

/// The name lookup's budget. The connect timeout's value, because a
/// name that takes longer to resolve is as dead as a connection that
/// never opens.
pub const DNS_TIMEOUT: Duration = CONNECT_TIMEOUT;

/// A client builder carrying the shared settings. An adapter adds its
/// user agent, headers, or per-request timeouts and builds.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .dns_resolver(Arc::new(BoundedResolver))
}

/// Resolves a name once, bounded at [`DNS_TIMEOUT`], and hands the
/// addresses to the connector. The lookup itself is the blocking
/// `getaddrinfo`; the timeout stops waiting for it, so the connection
/// fails at once while the lookup thread finishes in the background.
#[derive(Debug, Default)]
struct BoundedResolver;

impl Resolve for BoundedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = resolve(&host, DNS_TIMEOUT)
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// The lookup under `timeout`: the addresses a connector would try, or
/// the resolver's error.
async fn resolve(host: &str, timeout: Duration) -> io::Result<Vec<SocketAddr>> {
    let lookup = async {
        let addrs = tokio::net::lookup_host((host, 0)).await?;
        Ok(addrs.collect())
    };
    bounded(host, timeout, lookup).await
}

/// [`resolve`] with the lookup passed in: the timeout has to be testable
/// without a name the machine cannot answer.
async fn bounded<F>(host: &str, timeout: Duration, lookup: F) -> io::Result<Vec<SocketAddr>>
where
    F: std::future::Future<Output = io::Result<Vec<SocketAddr>>>,
{
    let addrs = match tokio::time::timeout(timeout, lookup).await {
        Ok(result) => result?,
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{host} did not resolve within {} s", timeout.as_secs()),
            ));
        }
    };
    if addrs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{host} resolved to no address"),
        ));
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_local_name_resolves_to_a_socket_address() {
        let addrs = resolve("localhost", Duration::from_secs(5))
            .await
            .expect("localhost resolves");
        assert!(!addrs.is_empty());
    }

    #[tokio::test]
    async fn a_lookup_past_its_budget_fails_as_a_timeout() {
        // A lookup that never answers stands in for a resolver the
        // machine's DNS configuration will not answer either.
        let err = bounded(
            "node304.lan",
            Duration::from_millis(20),
            std::future::pending(),
        )
        .await
        .expect_err("the budget ends the lookup");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "node304.lan did not resolve within 0 s");
    }

    #[tokio::test]
    async fn a_lookup_with_no_addresses_is_not_found() {
        let err = bounded("empty", Duration::from_secs(1), async { Ok(Vec::new()) })
            .await
            .expect_err("an empty answer names nothing to connect to");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
