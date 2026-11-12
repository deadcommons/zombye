//! The places of the Gear column (Linear DCM-43, #99): where each thing held and worn is listed.
//! [`Spot`]: the head, face, neck, torso, arms, hands, legs and feet for what is worn, and the two
//! hands, the back and the shoulder for what is held and carried; [`lines`] says what is at one,
//! outermost first.

use bevy::prelude::*;
use zombye_sim::{BodyPart, Inventory, ItemDef, ItemStack, Layer, Place, Slot, WearSlot};

use super::Line;

/// A place of the body view: where worn things are listed, or a slot that holds one thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Spot {
    Head,
    Face,
    Neck,
    Torso,
    Arms,
    Hands,
    Legs,
    Feet,
    Hand,
    OffHand,
    Back,
    Shoulder,
}

impl Spot {
    /// Every place.
    #[cfg(test)]
    pub(super) const ALL: [Spot; 12] = [Spot::Head, Spot::Face, Spot::Neck, Spot::Torso, Spot::Arms, Spot::Hands, Spot::Legs, Spot::Feet, Spot::Hand, Spot::OffHand, Spot::Back, Spot::Shoulder];

    pub(super) fn label(self) -> &'static str {
        match self {
            Spot::Head => "Head",
            Spot::Face => "Face",
            Spot::Neck => "Neck",
            Spot::Torso => "Torso",
            Spot::Arms => "Arms",
            Spot::Hands => "Hands",
            Spot::Legs => "Legs",
            Spot::Feet => "Feet",
            Spot::Hand => "Hand",
            Spot::OffHand => "Off hand",
            Spot::Back => "Back",
            Spot::Shoulder => "Shoulder",
        }
    }

    /// The inventory slot this place is, if it is one (else it lists worn things).
    pub(super) fn slot(self) -> Option<Slot> {
        match self {
            Spot::Hand => Some(Slot::Hand),
            Spot::OffHand => Some(Slot::OffHand),
            Spot::Back => Some(Slot::Back),
            Spot::Shoulder => Some(Slot::Shoulder),
            _ => None,
        }
    }

    /// Where a worn item is listed: the accessory place it takes (a hat on the head, glasses on
    /// the face, a scarf round the neck), else the main body part it covers (a hoodie the torso
    /// though it has arms, boots the feet though they reach up the legs). Anything else worn (a
    /// mod's item that covers nothing): the torso.
    pub(super) fn of(def: &ItemDef) -> Spot {
        let Some(c) = &def.clothing else { return Spot::Torso };
        for (takes, spot) in [(WearSlot::HeadTop, Spot::Head), (WearSlot::Face, Spot::Face), (WearSlot::Neck, Spot::Neck)] {
            if c.takes.contains(&takes) {
                return spot;
            }
        }
        for (covers, spot) in [(BodyPart::Torso, Spot::Torso), (BodyPart::Feet, Spot::Feet), (BodyPart::Hands, Spot::Hands), (BodyPart::Legs, Spot::Legs), (BodyPart::Arms, Spot::Arms), (BodyPart::Head, Spot::Head)] {
            if c.covers.contains(&covers) {
                return spot;
            }
        }
        Spot::Torso
    }
}

fn layer_of(def: Option<&ItemDef>) -> Layer {
    def.and_then(|d| d.clothing.as_ref()).map_or(Layer::default(), |c| c.layer)
}

/// What is at `spot`: the one thing in a slot, or the worn things listed there, outermost first
/// (a jacket over the hoodie over the T-shirt). Every held and worn stack is at exactly one place.
pub(super) fn lines(items: &[ItemDef], inv: &Inventory, spot: Spot) -> Vec<Line> {
    if let Some(slot) = spot.slot() {
        return inv.slot(slot).map(|stack| Line { from: Place::Slot(slot), index: 0, stack }).into_iter().collect();
    }
    let def = |s: &ItemStack| items.get(s.item as usize);
    let mut worn: Vec<Line> = inv.worn.iter().enumerate().filter(|(_, s)| def(s).map_or(Spot::Torso, Spot::of) == spot).map(|(k, s)| Line { from: Place::Worn, index: k as u32, stack: *s }).collect();
    worn.sort_by_key(|l| std::cmp::Reverse(layer_of(def(&l.stack))));
    worn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_items() -> Vec<ItemDef> {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        zombye_net::content::load_config(&data, 1).unwrap().items
    }

    fn stack(items: &[ItemDef], id: &str) -> ItemStack {
        ItemStack::one(items.iter().position(|d| d.id == id).unwrap_or_else(|| panic!("no item {id}")) as u16)
    }

    fn dressed(items: &[ItemDef]) -> Inventory {
        let s = |id: &str| stack(items, id);
        Inventory {
            hand: Some(s("crowbar")),
            off_hand: Some(s("flashlight")),
            back: Some(s("hiking_pack")),
            shoulder: Some(s("tote_bag")),
            worn: ["t_shirt", "jeans", "hoodie", "work_boots", "work_gloves", "glasses", "baseball_cap", "scarf", "leather_jacket"].into_iter().map(s).collect(),
            carried: vec![s("tinned_beans")],
        }
    }

    /// Every shipped thing that can be worn has its place, the places are each listed once,
    /// and every slot is one of them.
    #[test]
    fn every_slot_and_wearable_has_a_place() {
        let items = shipped_items();
        let all: Vec<Spot> = Spot::ALL.into_iter().collect();
        for (k, s) in all.iter().enumerate() {
            assert!(!all[..k].contains(s), "{s:?} twice");
        }
        for slot in [Slot::Hand, Slot::OffHand, Slot::Back, Slot::Shoulder] {
            assert_eq!(all.iter().filter(|s| s.slot() == Some(slot)).count(), 1, "{slot:?}");
        }
        let place = |id: &str| Spot::of(&items[stack(&items, id).item as usize]);
        assert_eq!(place("hoodie"), Spot::Torso);
        assert_eq!(place("jeans"), Spot::Legs);
        assert_eq!(place("work_boots"), Spot::Feet, "though they reach up the legs");
        assert_eq!(place("work_gloves"), Spot::Hands);
        assert_eq!(place("glasses"), Spot::Face);
        assert_eq!(place("dust_mask"), Spot::Face);
        assert_eq!(place("baseball_cap"), Spot::Head);
        assert_eq!(place("balaclava"), Spot::Head);
        assert_eq!(place("scarf"), Spot::Neck);
        for d in items.iter().filter(|d| d.clothing.is_some()) {
            let spot = Spot::of(d);
            assert!(spot.slot().is_none() && all.contains(&spot), "{}", d.id);
        }
    }

    /// Everything held and worn is at exactly one place, worn things outermost first; what is
    /// only carried is nowhere.
    #[test]
    fn everything_held_and_worn_is_at_one_place() {
        let items = shipped_items();
        let inv = dressed(&items);
        let all: Vec<Line> = Spot::ALL.into_iter().flat_map(|s| lines(&items, &inv, s)).collect();
        assert_eq!(all.len(), 4 + inv.worn.len());
        for slot in [Slot::Hand, Slot::OffHand, Slot::Back, Slot::Shoulder] {
            assert_eq!(all.iter().filter(|l| l.from == Place::Slot(slot)).count(), 1);
        }
        for k in 0..inv.worn.len() as u32 {
            assert_eq!(all.iter().filter(|l| l.from == Place::Worn && l.index == k).count(), 1, "worn {k}");
        }
        assert!(all.iter().all(|l| l.from != Place::Carried));
        let ids = |spot: Spot| lines(&items, &inv, spot).iter().map(|l| items[l.stack.item as usize].id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(Spot::Torso), ["leather_jacket", "hoodie", "t_shirt"], "outermost first");
        assert_eq!(ids(Spot::Hand), ["crowbar"]);
        assert_eq!(ids(Spot::Arms), Vec::<String>::new());
        // Their indices are the inventory's own (what the intents name).
        let jacket = lines(&items, &inv, Spot::Torso)[0];
        assert_eq!((jacket.from, jacket.index), (Place::Worn, 8));
        assert!(lines(&items, &Inventory::default(), Spot::Torso).is_empty() && lines(&items, &Inventory::default(), Spot::Back).is_empty());
    }
}
