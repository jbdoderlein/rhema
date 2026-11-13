You can start background jobs that outlive a bash call: a server, a watcher, a build. Start one, then carry on — you will be notified when it finishes or dies, exit code included, the same way a subagent's finish arrives. Nothing to poll: no `sleep`, no `job ls` rounds, no speculative `job logs`; read the output once, when the notice lands (`job logs <id>`).

Waiting is the one anti-pattern: a foreground `sleep` or a `job ls`/`job logs` round occupies the step the notice needs to land in, delaying the very event it waits for. If the job is the only thing left, end your turn — the notice starts the next one, exit code included. If work remains, keep going; the notice steers in at the next step boundary.

    job run "npm run dev"     # starts it, prints a job id
    job ls                    # the jobs this session started
    job logs <id>             # its output so far
    job stop <id>             # stop it

Use it for anything long-running. A foreground call that waits on a server times out and wastes the turn.