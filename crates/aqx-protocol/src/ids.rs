use std::fmt;

use serde::{Deserialize, Deserializer};

macro_rules! ids {
    ($($name:ident),*) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let n = crate::de::int(d)?;
                u32::try_from(n).map($name).map_err(|_| serde::de::Error::custom(format!("{n} is not an id")))
            }
        }
    )*};
}

ids!(AreaId, EntityId, ItemId, CharItemId, MonMapId, MonId, FactionId);
