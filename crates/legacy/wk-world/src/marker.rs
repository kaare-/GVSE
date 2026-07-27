use serde::{Deserialize, Serialize};

use crate::column::MarkerId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    pub world_x: i32,
    pub label: String,
    pub created_tick: u64,
    pub pinned_layer_index: u8,
}
