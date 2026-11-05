use std::task::Poll;
use std::time::Instant;

use mira_protocol::{CharItemId, ItemId, ShopItemId, packet};

use super::{ActionError, Limit, Op, Session};
use crate::REPLY_TIMEOUT;
use crate::inventory::Pool;
use crate::quest::Held;
use crate::shop::{Bought, ShopItem};

pub(super) enum Trade {
    Buy { listing: ShopItemId, qty: u32 },
    Sell { item: ItemId, qty: u32 },
}

pub(super) enum TradeStage {
    Waiting {
        wake: Option<Instant>,
    },
    Bought {
        listed: Box<ShopItem>,
        deadline: Instant,
    },
    Sold {
        char_item_id: CharItemId,
        deadline: Instant,
    },
}

impl TradeStage {
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            TradeStage::Waiting { wake } => *wake,
            TradeStage::Bought { deadline, .. } | TradeStage::Sold { deadline, .. } => {
                Some(*deadline)
            }
        }
    }
}

impl Session {
    pub(super) fn step_trade(
        &mut self,
        trade: &Trade,
        stage: &mut TradeStage,
        now: Instant,
    ) -> Poll<Result<(), ActionError>> {
        if let Err(e) = self.ensure_ready() {
            return Poll::Ready(Err(e));
        }
        if let TradeStage::Bought { deadline, .. } | TradeStage::Sold { deadline, .. } = *stage {
            return if now >= deadline {
                Poll::Ready(Err(ActionError::NoReply))
            } else {
                Poll::Pending
            };
        }
        let prepared = match *trade {
            Trade::Buy { listing, qty } => self.prepare_buy(listing, qty),
            Trade::Sell { item, qty } => self.prepare_sell(item, qty),
        };
        let (limit, packet, sent) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => return Poll::Ready(Err(e)),
        };
        if let Some(until) = self.cooling(limit, now) {
            *stage = TradeStage::Waiting { wake: Some(until) };
            return Poll::Pending;
        }
        self.limits.used(limit, now);
        self.send(&packet);
        let deadline = now + REPLY_TIMEOUT;
        *stage = match sent {
            Sent::Buy(listed) => TradeStage::Bought { listed, deadline },
            Sent::Sell(char_item_id) => TradeStage::Sold {
                char_item_id,
                deadline,
            },
        };
        Poll::Pending
    }

    fn prepare_buy(
        &self,
        listing: ShopItemId,
        qty: u32,
    ) -> Result<(Limit, String, Sent), ActionError> {
        let unusable = |why: String| Err(ActionError::Unusable(why));
        let (Some(me), Some(room)) = (&self.character, &self.room) else {
            return Err(ActionError::NotReady);
        };
        let Some(shop) = &self.shop else {
            return unusable("load a shop first".into());
        };
        let Some(listed) = shop.items.iter().find(|i| i.listing == listing) else {
            return Err(ActionError::NoTarget(format!(
                "shop {} has no listing {listing}",
                shop.id
            )));
        };
        let name = &listed.name;
        if listed.member && !me.member {
            return unusable(format!("{name} is for members"));
        }
        if listed.quest_locked(me) {
            return unusable(format!("{name} needs more quest progress"));
        }
        let held = Held {
            items: &self.inventory.items,
            temp: &self.inventory.temp,
        };
        if let Some((token, needed)) = listed.tokens_needed(qty)
            && u64::from(held.qty(token).unwrap_or(0)) < needed
        {
            return unusable(format!("{name} needs {needed} of item {token}"));
        }
        if shop.merges() && self.inventory.full_for_purchase(listed, qty, me) {
            let pool = match listed.pool() {
                Pool::House => "house inventory",
                Pool::Misc => "misc inventory",
                _ => "inventory",
            };
            return unusable(format!("the {pool} has no room for {name}"));
        }
        let most = listed.most_buyable(me, held);
        if most == 0 {
            return unusable(format!(
                "{name} can't be bought now: it costs more than you have, its stack is full, \
                 or a merge item is missing"
            ));
        }
        let fixed = most < 2 || (listed.coins && listed.cost > 0);
        let fits = if fixed {
            qty == listed.qty
        } else {
            (listed.qty..=most).contains(&qty)
        };
        if !fits {
            return unusable(if fixed {
                format!("{name} sells only as {}", listed.qty)
            } else {
                format!("{name} sells {} to {most} at a time now", listed.qty)
            });
        }
        let packet = packet::buy_item(room.area, listed.id, shop.id, listing, qty);
        Ok((Limit::BuyItem, packet, Sent::Buy(Box::new(listed.clone()))))
    }

    fn prepare_sell(&self, item: ItemId, qty: u32) -> Result<(Limit, String, Sent), ActionError> {
        let Some(room) = &self.room else {
            return Err(ActionError::NotReady);
        };
        if self.shop.is_none() {
            return Err(ActionError::Unusable("load a shop first".into()));
        }
        let Some(held) = self.inventory.items.iter().find(|i| i.id == item) else {
            return Err(ActionError::NoTarget(format!(
                "item {item} isn't in the inventory"
            )));
        };
        let most = if held.slot == "ar" { 1 } else { held.qty };
        if !(1..=most).contains(&qty) {
            return Err(ActionError::Unusable(format!(
                "{} sells 1 to {most} at a time",
                held.name
            )));
        }
        let packet = packet::sell_item(room.area, item, qty, held.char_item_id);
        Ok((Limit::SellItem, packet, Sent::Sell(held.char_item_id)))
    }

    pub(super) fn on_bought(&mut self, bought: Bought) {
        let Some(Op::Trade {
            stage: TradeStage::Bought { listed, .. },
            ..
        }) = &self.current
        else {
            return;
        };
        if bought.id.is_some_and(|id| id != listed.id) {
            return;
        }
        let Some(Op::Trade {
            reply,
            stage: TradeStage::Bought { listed, .. },
            ..
        }) = self.current.take()
        else {
            return;
        };
        let result = if bought.ok {
            if let Some(me) = &mut self.character {
                me.spend(listed.cost * u64::from(bought.qty), listed.coins);
            }
            if let Some(event) = self.inventory.buy(&listed, &bought) {
                self.emit(event);
            }
            Ok(())
        } else {
            let why = bought.message.filter(|m| !m.is_empty());
            Err(ActionError::Refused(why.unwrap_or_else(|| {
                format!("the shop wouldn't sell {}", listed.name)
            })))
        };
        self.replies.push((reply, result));
    }

    pub(super) fn on_sold(&mut self, char_item_id: CharItemId) {
        let matches = matches!(
            &self.current,
            Some(Op::Trade { stage: TradeStage::Sold { char_item_id: sold, .. }, .. })
                if *sold == char_item_id
        );
        if matches && let Some(Op::Trade { reply, .. }) = self.current.take() {
            self.replies.push((reply, Ok(())));
        }
    }
}

enum Sent {
    Buy(Box<ShopItem>),
    Sell(CharItemId),
}
