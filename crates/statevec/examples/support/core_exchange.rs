//! Small order/reservation Core example for native-host qualification. It is
//! not a matching service or a migration of an application's exchange schema.
use statevec::api::v1::*;
use statevec::prelude::*;

#[schema_module(version = "1.0")]
mod exchange {
    use super::*;
    #[record(kind = 1, record_len = 64, unique_key(id = 0, fields = [trader]))]
    pub struct Position {
        #[field(index = 1, immutable = true)] pub trader: u64,
        #[field(index = 2)] pub available: u64,
        #[field(index = 3)] pub reserved: u64,
    }
    #[record(kind = 2, record_len = 64, unique_key(id = 0, fields = [order_id]), index(id = 0, name = "by_trader", fields = [trader]))]
    pub struct Order {
        #[field(index = 1, immutable = true)] pub order_id: u64,
        #[field(index = 2, immutable = true)] pub trader: u64,
        #[field(index = 3, immutable = true)] pub price: u64,
        #[field(index = 4)] pub remaining: u64,
    }
    #[command(kind = 1)]
    pub struct Credit {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub amount: u64,
    }
    #[command(kind = 2)]
    pub struct Place {
        #[field(index = 1)] pub order_id: u64,
        #[field(index = 2)] pub trader: u64,
        #[field(index = 3)] pub price: u64,
        #[field(index = 4)] pub quantity: u64,
    }
    #[command(kind = 3)]
    pub struct Cancel {
        #[field(index = 1)] pub order_id: u64,
    }
    #[command(kind = 4)]
    pub struct Fill {
        #[field(index = 1)] pub order_id: u64,
        #[field(index = 2)] pub quantity: u64,
    }
    #[command(kind = 5)]
    pub struct Audit {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub cap: u64,
        #[field(index = 3)] pub panic_in_visitor: u64,
    }
    #[event(kind = 2, inline_response = true)]
    pub struct Audited {
        #[field(index = 1)] pub visited: u64,
        #[field(index = 2)] pub reserved: u64,
        #[field(index = 3)] pub count: u64,
        #[field(index = 4)] pub at_least: u64,
    }
    #[event(kind = 1, inline_response = true)]
    pub struct Changed {
        #[field(index = 1)] pub trader: u64,
        #[field(index = 2)] pub available: u64,
        #[field(index = 3)] pub reserved: u64,
        #[field(index = 4)] pub order_id: u64,
        #[field(index = 5)] pub remaining: u64,
        #[field(index = 6)] pub time_ns: u64,
        #[field(index = 7)] pub position: u64,
    }
}
use exchange::*;
pub struct Module<const RULES: bool>;
pub struct Configured<const RULES: bool>(SchemaRegistry);
pub struct Runtime<const RULES: bool>;
fn reject(code: u16) -> CoreTxError {
    CoreTxError::Rejected(BusinessRejectCode::new(code).unwrap())
}
impl<const RULES: bool> CoreModule for Module<RULES> {
    type Configured = Configured<RULES>;
    fn configure(&self, config: CoreConfig<'_>) -> Result<Self::Configured, ConfigureError> {
        if config.bytes != b"{}" { return Err(ConfigureError::Invalid { code: 1 }); }
        Ok(Configured(registry()))
    }
}
impl<const RULES: bool> ConfiguredCore for Configured<RULES> {
    type Runtime = Runtime<RULES>;
    fn metadata(&self) -> CoreMetadata<'_> {
        CoreMetadata { name: "core-exchange", revision: if RULES { "core-exchange-v2;notional=100" } else { "core-exchange-v2;no-rules" }, schema: &self.0 }
    }
    fn create_instance(&self) -> Result<Self::Runtime, CoreCreateError> { Ok(Runtime) }
}
impl<const RULES: bool> Runtime<RULES> {
    fn audit(tx: &mut PluginTx<'_>, payload: &[u8]) -> Result<(), CoreTxError> {
        let command = Audit::wrap(payload);
        let mut visited = 0;
        let mut reserved = 0;
        let mut failure = None;
        TxReadContext::for_each_record_key(tx, Order::KIND, &mut |key| {
            if failure.is_some() { return; }
            let read = tx.with_read_typed::<Order, _, _>(key.sys_id, |order| {
                // Shared reentry while both the walk and this read are live.
                let nested = TxReadContext::count_index_prefix_capped_raw(tx, Order::KIND, 0, &[], 0)?;
                assert_eq!(nested, statevec::api::CanonicalIndexCount::AtLeast(0));
                assert_eq!(command.panic_in_visitor(), 0, "deliberate loaded exchange visitor panic");
                Ok::<_, CoreExecutionError>(order.price() * order.remaining())
            });
            match read {
                Ok(Some(Ok(value))) => { visited += 1; reserved += value; }
                Ok(Some(Err(error))) | Err(error) => failure = Some(error),
                Ok(None) => panic!("walk delivered a missing order"),
            }
        })?;
        if let Some(error) = failure { return Err(error.into()); }
        let count = TxReadContext::count_index_prefix_capped_raw(tx, Order::KIND, 0, &command.trader().to_be_bytes(), command.cap() as usize)?;
        let (count, at_least) = match count {
            statevec::api::CanonicalIndexCount::Exact(n) => (n, 0),
            statevec::api::CanonicalIndexCount::AtLeast(n) => (n, 1),
        };
        tx.emit_typed_event::<Audited>(Audited::builder().set_visited(visited).set_reserved(reserved)
            .set_count(count as u64).set_at_least(at_least).build().unwrap());
        Ok(())
    }
    fn position(tx: &PluginTx<'_>, trader: u64) -> Result<(u64, u64), CoreTxError> {
        tx.with_read_typed_by_unique_key::<Position, _, _, _>(Position::unique_key(trader), |p| (p.available(), p.reserved()))?
            .ok_or_else(|| reject(2))
    }
    fn write_position(tx: &mut PluginTx<'_>, trader: u64, available: u64, reserved: u64) -> Result<(), CoreTxError> {
        tx.update_or_create_typed_by_unique_key::<Position, _, _, _, _>(Position::unique_key(trader),
            |p| { p.set_available(available); p.set_reserved(reserved); },
            |p| { p.init_trader(trader); p.set_available(available); p.set_reserved(reserved); })?;
        Ok(())
    }
    fn emit(tx: &mut PluginTx<'_>, trader: u64, order: u64, remaining: u64) -> Result<(), CoreTxError> {
        let (available, reserved) = Self::position(tx, trader)?;
        tx.emit_typed_event::<Changed>(Changed::builder().set_trader(trader).set_available(available)
            .set_reserved(reserved).set_order_id(order).set_remaining(remaining)
            .set_time_ns(tx.ref_tx_time_ns()?).set_position(tx.tx_seq()?).build().unwrap());
        Ok(())
    }
    fn rules(&self, view: &dyn CandidateRead) -> Result<(), CoreTxError> {
        let mut rejected = false;
        view.for_each_after_change(&mut |change| {
            let (key, after) = match change {
                CandidateAfterChange::New { key, after } | CandidateAfterChange::Update { key, after } => (key, after),
                CandidateAfterChange::Delete { .. } => return,
            };
            if key.kind == Order::KIND {
                let row = Order::wrap(after);
                rejected |= row.price().checked_mul(row.remaining()).is_none_or(|n| n > 100);
            }
        })?;
        if rejected { Err(reject(4)) } else { Ok(()) }
    }
}
impl<const RULES: bool> CoreRuntime for Runtime<RULES> {
    const TX_INVARIANTS: Option<TxInvariantHook<Self>> = if RULES { Some(Self::rules) } else { None };
    fn preflight_command(&self, command: &RuntimeCommandRef<'_>) -> Result<(), statevec::model::CommandSchemaError> {
        match command.command_kind() {
            Credit::KIND => Credit::validate_payload(command.payload()),
            Place::KIND => Place::validate_payload(command.payload()),
            Cancel::KIND => Cancel::validate_payload(command.payload()),
            Fill::KIND => Fill::validate_payload(command.payload()),
            Audit::KIND => Audit::validate_payload(command.payload()),
            kind => Err(statevec::model::CommandSchemaError::UnsupportedKind { kind }),
        }
    }
    fn run_tx(&self, tx: &mut PluginTx<'_>, command: &RuntimeCommandRef<'_>) -> Result<(), CoreTxError> {
        match command.command_kind() {
            Audit::KIND => Self::audit(tx, command.payload()),
            Credit::KIND => {
                let c = Credit::wrap(command.payload());
                let (available, reserved) = tx.with_read_typed_by_unique_key::<Position, _, _, _>(Position::unique_key(c.trader()),
                    |p| (p.available(), p.reserved()))?.unwrap_or((0, 0));
                Self::write_position(tx, c.trader(), available.checked_add(c.amount()).ok_or_else(|| reject(3))?, reserved)?;
                Self::emit(tx, c.trader(), 0, 0)
            }
            Place::KIND => {
                let c = Place::wrap(command.payload());
                if c.quantity() == 0 || c.price() == 0 || tx.resolve_unique_key(Order::KIND, Order::unique_key(c.order_id()).as_ref())?.is_some() {
                    return Err(reject(1));
                }
                let cost = c.price().checked_mul(c.quantity()).ok_or_else(|| reject(3))?;
                let (available, reserved) = Self::position(tx, c.trader())?;
                Self::write_position(tx, c.trader(), available.checked_sub(cost).ok_or_else(|| reject(2))?,
                    reserved.checked_add(cost).ok_or_else(|| reject(3))?)?;
                tx.create_typed::<Order, _>(|o| { o.init_order_id(c.order_id()); o.init_trader(c.trader());
                    o.init_price(c.price()); o.set_remaining(c.quantity()); })?;
                Self::emit(tx, c.trader(), c.order_id(), c.quantity())
            }
            Cancel::KIND | Fill::KIND => {
                let order_id = Cancel::wrap(command.payload()).order_id();
                let (trader, price, remaining) = tx.with_read_typed_by_unique_key::<Order, _, _, _>(Order::unique_key(order_id),
                    |o| (o.trader(), o.price(), o.remaining()))?.ok_or_else(|| reject(1))?;
                let cancel = command.command_kind() == Cancel::KIND;
                let filled = if cancel { remaining } else { Fill::wrap(command.payload()).quantity() };
                if filled == 0 || filled > remaining { return Err(reject(1)); }
                let release = price.checked_mul(filled).ok_or_else(|| reject(3))?;
                let (available, reserved) = Self::position(tx, trader)?;
                Self::write_position(tx, trader, if cancel { available.checked_add(release).ok_or_else(|| reject(3))? } else { available },
                    reserved.checked_sub(release).ok_or_else(|| reject(3))?)?;
                let left = remaining - filled;
                if left == 0 { tx.delete_by_unique_key::<Order, _>(Order::unique_key(order_id))?; }
                else { tx.update_typed_by_unique_key::<Order, _, _, _>(Order::unique_key(order_id), |o| { o.set_remaining(left); })?; }
                Self::emit(tx, trader, order_id, left)
            }
            kind => Err(CoreExecutionError::Schema(CoreSchemaError::UnsupportedKind { kind }).into()),
        }
    }
}
