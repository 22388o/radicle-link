// Copyright © 2021 The Radicle Link Contributors
//
// This file is part of radicle-link, distributed under the GPLv3 with Radicle
// Linking Exception. For full terms see the included LICENSE file.

use link_async::Spawner;
use tokio::runtime::Runtime;

mod tasks;

#[test]
#[should_panic(expected = "task has failed")]
fn unhelpful_panic() {
    Runtime::new().unwrap().block_on(async {
        let spawner = Spawner::from_current().unwrap();
        spawner
            .blocking(|| panic!("you won't see this unless `--nocapture`"))
            .await
    })
}
