use wk_material::CHUNK_W;

use crate::column::{Activity, Column, SedimentLoad};

#[derive(Debug, Clone)]
pub struct ChunkInbox {
    pub water_in: [i64; CHUNK_W],
    pub sediment_in: [SedimentLoad; CHUNK_W],
    pub moisture_in: [i64; CHUNK_W],
}

impl Default for ChunkInbox {
    fn default() -> Self {
        Self {
            water_in: [0; CHUNK_W],
            sediment_in: [SedimentLoad::default(); CHUNK_W],
            moisture_in: [0; CHUNK_W],
        }
    }
}

impl ChunkInbox {
    pub fn clear(&mut self) {
        self.water_in = [0; CHUNK_W];
        self.sediment_in = [SedimentLoad::default(); CHUNK_W];
        self.moisture_in = [0; CHUNK_W];
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub coord: i32,
    pub columns: [Column; CHUNK_W],
    pub bedrock_y: f32,
    pub inbox: ChunkInbox,
    pub halo_surface_y: [f32; 2],
    pub halo_water: [i64; 2],
    pub halo_water_table: [f32; 2],
}

impl Chunk {
    pub fn new(coord: i32, bedrock_y: f32) -> Self {
        Self {
            coord,
            columns: std::array::from_fn(|_| Column::default()),
            bedrock_y,
            inbox: ChunkInbox::default(),
            halo_surface_y: [0.0, 0.0],
            halo_water: [0, 0],
            halo_water_table: [0.0, 0.0],
        }
    }

    pub fn world_x_base(&self) -> i32 {
        self.coord * CHUNK_W as i32
    }

    pub fn surface_y_at(&self, local_x: usize) -> f32 {
        self.columns[local_x].surface_y
    }

    pub fn surface_y_neighbor(&self, local_x: i32) -> f32 {
        if local_x < 0 {
            self.halo_surface_y[0]
        } else if local_x >= CHUNK_W as i32 {
            self.halo_surface_y[1]
        } else {
            self.columns[local_x as usize].surface_y
        }
    }

    pub fn water_neighbor(&self, local_x: i32) -> i64 {
        if local_x < 0 {
            self.halo_water[0]
        } else if local_x >= CHUNK_W as i32 {
            self.halo_water[1]
        } else {
            self.columns[local_x as usize].top_water_mass()
        }
    }

    pub fn water_table_neighbor(&self, local_x: i32) -> f32 {
        if local_x < 0 {
            self.halo_water_table[0]
        } else if local_x >= CHUNK_W as i32 {
            self.halo_water_table[1]
        } else {
            self.columns[local_x as usize].water_table_y()
        }
    }

    pub fn update_halos_from_neighbors(
        &mut self,
        left: Option<&Chunk>,
        right: Option<&Chunk>,
    ) {
        self.halo_surface_y[0] = left
            .map(|c| c.columns[CHUNK_W - 1].surface_y)
            .unwrap_or(self.columns[0].surface_y);
        self.halo_water[0] = left
            .map(|c| c.columns[CHUNK_W - 1].top_water_mass())
            .unwrap_or(0);
        self.halo_water_table[0] = left
            .map(|c| c.columns[CHUNK_W - 1].water_table_y())
            .unwrap_or_else(|| self.columns[0].water_table_y());
        self.halo_surface_y[1] = right
            .map(|c| c.columns[0].surface_y)
            .unwrap_or(self.columns[CHUNK_W - 1].surface_y);
        self.halo_water[1] = right
            .map(|c| c.columns[0].top_water_mass())
            .unwrap_or(0);
        self.halo_water_table[1] = right
            .map(|c| c.columns[0].water_table_y())
            .unwrap_or_else(|| self.columns[CHUNK_W - 1].water_table_y());
    }

    pub fn any_hydrology_active(&self) -> bool {
        self.columns
            .iter()
            .any(|c| c.activity == Activity::HydrologyActive)
    }

    pub fn set_all_active(&mut self) {
        for col in &mut self.columns {
            col.activity = Activity::HydrologyActive;
        }
    }

}
