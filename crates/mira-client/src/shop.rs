use mira_protocol::{FactionId, ItemId, ShopId, ShopItemId, flex};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shop {
    #[serde(rename(deserialize = "ShopID"))]
    pub id: ShopId,
    #[serde(rename(deserialize = "sName"), default)]
    pub name: String,
    #[serde(
        rename(deserialize = "bUpgrd"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub member: bool,
    #[serde(
        rename(deserialize = "bLimited"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub limited: bool,
    #[serde(rename(deserialize = "Location"), default, deserialize_with = "maps")]
    pub maps: Vec<String>,
    #[serde(rename(deserialize = "items"), default)]
    pub items: Vec<ShopItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopItem {
    #[serde(rename(deserialize = "ShopItemID"))]
    pub listing: ShopItemId,
    #[serde(rename(deserialize = "ItemID"))]
    pub id: ItemId,
    #[serde(rename(deserialize = "sName"), default)]
    pub name: String,
    #[serde(rename(deserialize = "iCost"), default, deserialize_with = "flex::int")]
    pub cost: u64,
    #[serde(
        rename(deserialize = "bCoins"),
        default,
        deserialize_with = "flex::bool"
    )]
    pub coins: bool,
    #[serde(rename(deserialize = "bUpg"), default, deserialize_with = "flex::bool")]
    pub member: bool,
    #[serde(rename(deserialize = "iStk"), default, deserialize_with = "flex::int")]
    pub max_stack: u32,
    #[serde(rename(deserialize = "iQty"), default, deserialize_with = "flex::int")]
    pub qty: u32,
    #[serde(
        rename(deserialize = "FactionID"),
        default,
        deserialize_with = "faction"
    )]
    pub faction: Option<FactionId>,
    #[serde(
        rename(deserialize = "iReqRep"),
        default,
        deserialize_with = "flex::int"
    )]
    pub required_rep: u32,
    #[serde(rename(deserialize = "sType"), default)]
    pub kind: String,
    #[serde(rename(deserialize = "sES"), default)]
    pub slot: String,
}

fn maps<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let list = String::deserialize(d)?;
    Ok(list
        .split(',')
        .map(str::trim)
        .filter(|map| !map.is_empty())
        .map(str::to_owned)
        .collect())
}

fn faction<'de, D: Deserializer<'de>>(d: D) -> Result<Option<FactionId>, D::Error> {
    let id: i64 = flex::int(d)?;
    Ok((id > 1).then_some(FactionId(id)))
}

#[derive(Deserialize)]
pub(crate) struct LoadedShop {
    pub(crate) shopinfo: Shop,
}
