//! Delegate-mode types of the client API (proxy delegation contracts-v1
//! §15.1), bridged to Dart unchanged. Times are unix seconds, and
//! `options[i]` is the choice on proposal `i + 1`.

use serde::{Deserialize, Serialize};

/// The identity provider of a delegate entry's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProxyDelegateProvider {
    /// X, wire code 1.
    #[serde(rename = "x")]
    X,
    /// GitHub, wire code 2.
    #[serde(rename = "github")]
    GitHub,
}
