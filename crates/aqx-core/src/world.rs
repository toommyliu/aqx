use aqx_protocol::{AreaId, EntityId};

#[derive(Default)]
pub struct World {
    pub me: Option<Me>,
    pub area: Option<Area>,
    pub others: Vec<EntityId>,
    pub items: usize,
}

pub struct Me {
    pub eid: EntityId,
    pub user: String,
    pub char_id: Option<u32>,
}

pub struct Area {
    pub id: AreaId,
    pub name: String,
    pub map: String,
}
