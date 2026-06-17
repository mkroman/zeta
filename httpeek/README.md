# httpeek

HTTPeek is a fast, async, lock-less URL meta-info fetcher.

Given a HTTP(S) URL, it will request the given URL, imitating a regular browser,
and extract the meta-info available on the page typically used by search engines
(`<meta>` tags), SoMe (Open Graph Protocol and more).

## Features

Currently implemented features include:

- None

## Planned

- [ ] Title extraction
- [ ] [Open Graph Protocol] extraction
- [ ] "Immediate" result - users can specify the data to extract (title, ogp,
  ..) and the order of preference, and as soon as that data is available, return
  it immediately
- [ ] Mastodon instance discovery and rich data extraction
- [ ] Bluesky instance discovery and rich data extraction
- [ ] Other ActivityPub instances and rich data extraction

[Open Graph Protocol]: https://ogp.me/

## Non-goals

* Page content extraction or classification
