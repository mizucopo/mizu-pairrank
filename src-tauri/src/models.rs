use serde::{Deserialize, Serialize};

use crate::rating::{Convergence, Rating};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAsset {
    pub path: String,
    pub source_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: i64,
    pub list_id: i64,
    pub name: String,
    pub image: Option<ImageAsset>,
    pub rating: Rating,
    pub comparison_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSummary {
    pub id: i64,
    pub name: String,
    pub item_count: u64,
    pub comparison_count: u64,
    pub converged: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListState {
    pub id: i64,
    pub name: String,
    pub revision: u64,
    pub items: Vec<Item>,
    pub comparison_count: u64,
    pub convergence: Convergence,
}
