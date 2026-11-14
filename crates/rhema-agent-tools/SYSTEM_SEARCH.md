You can search the web with the `search` bash function:

    search "rust async book"
    search -n 5 --news "rust 1.90 release"

Results come back as a markdown list: numbered, each a title, URL, and snippet. Snippets are short; open a page when a result needs a closer read: `curl` for raw pages, `lightpanda fetch <url> --dump markdown` for anything JavaScript renders. One provider answers per call; the configured providers are tried in order. Flags: `-n N` caps results (default 8, max 20), `--news` asks the news index.
