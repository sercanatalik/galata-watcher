//! The account's fills, read with the fields the watcher needs.
//!
//! galata-desk-core's `UserFill` drops the `liquidation` object, which is the
//! only mark of a liquidation fill (its `dir` stays `Close Long`). So the
//! watcher asks for fills itself, through desk-core's `Transport`, and keeps
//! that object (design decision 9).

use galata_desk_core::venue::{Transport, VenueError};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fill {
    pub coin: String,
    pub px: Decimal,
    pub sz: Decimal,
    /// `B` bid, `A` ask.
    pub side: String,
    pub time: u64,
    pub tid: u64,
    pub oid: u64,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub liquidation: Option<Liquidation>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Liquidation {
    pub liquidated_user: String,
}

impl Fill {
    /// This fill liquidated `account`. A liquidator's fill carries the same
    /// object, naming somebody else.
    pub fn liquidates(&self, account: &str) -> bool {
        self.liquidation
            .as_ref()
            .is_some_and(|l| l.liquidated_user.eq_ignore_ascii_case(account))
    }
}

/// The account's fills from `start_ms` on, oldest first, at most 2,000.
pub async fn fills_since<T: Transport>(transport: &T, user: &str, start_ms: u64) -> Result<Vec<Fill>, VenueError> {
    let body = json!({ "type": "userFillsByTime", "user": user, "startTime": start_ms });
    let bytes = transport.post(body).await?;
    let mut fills: Vec<Fill> = serde_json::from_slice(&bytes).map_err(|e| VenueError::Shape {
        query: "userFillsByTime",
        path: String::new(),
        detail: e.to_string(),
    })?;
    fills.sort_by_key(|f| (f.time, f.tid));
    Ok(fills)
}
