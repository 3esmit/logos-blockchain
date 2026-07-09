use crate::mantle::{
    channel::{SlotTimeframe, SlotTimeout},
    nom::nom_wire_fixtures,
};

nom_wire_fixtures!(SlotTimeframe, Self::from(0x0403_0201u32) => "01020304");
nom_wire_fixtures!(SlotTimeout, Self::from(0x0403_0201u32) => "01020304");
