// Copyright © 2021 The Radicle Link Contributors
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use linkd_lib::node::run;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("linkd failed: {:?}", e);
    }
}
