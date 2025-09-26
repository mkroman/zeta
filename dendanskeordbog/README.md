# dendanskeordbog

This is a Rust crate that implements a client and parser for the Danish
Dictionary (Den Danske Ordbog).

It relies on the mobile-oriented text/html http endpoint that has been
reverse-engineered from the [official DDO app][ddo app].

[ddo app]: https://ordnet.dk/ddo/app

## Features

* Efficiently reuse data structures (CSS selectors are parsed and cached during
  each invocation)
* Optional `client` feature for Client HTTP API
* Optional `serde` feature for serialization/deserialization.

## Quick Start

To get started, add this crate to your `Cargo.toml`. The main entry point for
fetching data is the [`Client::query`] method.

```rust
use dendanskeordbog::{Client, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Create a new client with default settings.
    let client = Client::new();

    // Query the dictionary for the word "hest".
    let document = client.query("hest").await?;

    // A document can contain multiple entries (e.g., for homonyms).
    if let Some(entry) = document.entries.first() {
        println!("Found entry for: {}", entry.head.keyword);
        println!("Part of speech: {}", entry.pos);

        if let Some(definition) = entry.definitions.first() {
            println!("First definition: {}", definition.description);
        }
    }

    Ok(())
}
```
