//! Shared compile-time owner roles and executable-evidence metadata.
//!
//! `#[semantic_contract_v1(owner = Owner, requires = [OwnerPath, PhysicalIt])]`
//! declares evidence requirements on an existing owner method, not a new
//! transition wrapper. `#[semantic_evidence(transition = Owner::step,
//! kind = OwnerPath)]` precedes an ordinary `#[test]`. The shared Dylint resolves
//! definitions and call edges; the repository runner requires exact nonzero
//! execution. Metadata alone does not prove the tested semantic assertion.

extern crate self as statevec_domain_roles;

pub use statevec_domain_role_macros::{
    domain_actor, domain_adapter_module, domain_core_state, domain_effect, domain_error_mapper, domain_event,
    domain_failure, domain_feedback, domain_initial_owner_creation, domain_invariant, domain_observation,
    domain_owner_module, domain_port_module, domain_projection, domain_record, domain_rejection,
    domain_resource_terminal, domain_result, domain_transition, evidence_pending_foundation, exclusive_domain_owner,
    move_only_resource, physical_observation_minter, physical_worker, production_bound_stateright_model,
    ratcheted_owner, semantic_contract_v1, semantic_evidence, shipping_adapter, stateright_model,
};

pub trait DomainActorRole {}
pub trait DomainEventRole {}
pub trait ExclusiveDomainOwnerRole {}
pub trait RatchetedOwnerRole {}
pub trait PhysicalWorkerRole {}
pub trait ShippingAdapterRole {}
pub trait DomainEffectRole {}
pub trait DomainFeedbackRole {}
pub trait DomainFailureRole {}
pub trait DomainRejectionRole {}
pub trait ClosedDomainResultRole {}
pub trait DomainObservationRole {}
pub trait DomainProjectionRole {}
pub trait DomainCoreStateRole {}
/// Shared semantic values whose fields must preserve nominal identity types.
pub trait DomainRecordRole {}
pub trait StaterightModelRole {}
pub trait ProductionBoundStaterightModelRole: StaterightModelRole {}
pub trait EvidencePendingFoundationRole {}
pub trait MoveOnlyResourceRole {}

#[doc(hidden)]
#[inline(always)]
pub const fn __domain_transition_marker() {}

#[doc(hidden)]
#[inline(always)]
pub const fn __domain_resource_terminal_marker() {}
#[doc(hidden)]
#[inline(always)]
pub const fn __domain_initial_owner_creation_marker() {}
#[doc(hidden)]
#[inline(always)]
pub const fn __physical_observation_minter_marker() {}

#[doc(hidden)]
#[inline(always)]
pub const fn __domain_invariant_marker(_: &'static str) {}

#[doc(hidden)]
#[inline(always)]
pub const fn __domain_error_mapper_marker(_: &'static str) {}

#[doc(hidden)]
#[inline(always)]
pub fn __semantic_contract_v1_marker<O: ExclusiveDomainOwnerRole>(_: u8) {}

#[doc(hidden)]
#[inline(always)]
pub fn __semantic_evidence_marker<F>(_: F, _: u8) {}

#[cfg(test)]
mod tests {
    use super::{
        ClosedDomainResultRole, DomainActorRole, DomainCoreStateRole, DomainEffectRole, DomainEventRole,
        DomainFailureRole, DomainFeedbackRole, DomainObservationRole, DomainProjectionRole, DomainRejectionRole,
        EvidencePendingFoundationRole, ExclusiveDomainOwnerRole, MoveOnlyResourceRole, PhysicalWorkerRole,
        ProductionBoundStaterightModelRole, RatchetedOwnerRole, ShippingAdapterRole, StaterightModelRole, domain_actor,
        domain_adapter_module, domain_core_state, domain_effect, domain_error_mapper, domain_event, domain_failure,
        domain_feedback, domain_initial_owner_creation, domain_invariant, domain_observation, domain_owner_module,
        domain_port_module, domain_projection, domain_rejection, domain_resource_terminal, domain_result,
        evidence_pending_foundation, exclusive_domain_owner, move_only_resource, physical_observation_minter,
        physical_worker, production_bound_stateright_model, ratcheted_owner, shipping_adapter, stateright_model,
    };

    #[domain_actor]
    struct ExampleOwner<T>(T);
    #[domain_event]
    enum ExampleEvent {
        Start,
    }
    #[exclusive_domain_owner]
    struct ExampleExclusiveOwner;
    #[ratcheted_owner]
    struct ExampleRatchetedOwner;
    #[physical_worker]
    struct ExampleWorker;
    #[shipping_adapter]
    struct ExampleAdapter;
    #[domain_effect]
    enum ExampleEffect {
        Submit,
    }
    #[domain_feedback]
    enum ExampleFeedback {
        Returned,
    }
    #[domain_failure]
    enum ExampleFailure {
        Unavailable,
    }
    #[domain_rejection]
    enum ExampleRejection {
        NotAdmitted,
    }
    #[domain_result]
    enum ExampleResult {
        Complete,
    }
    #[domain_observation]
    enum ExampleObservation {
        Accepted,
    }
    #[domain_projection]
    enum ExampleProjection {
        Current,
    }
    #[domain_core_state]
    enum ExampleCoreState {
        Current,
    }
    #[super::domain_record]
    struct ExampleRecord<T>(T);
    #[domain_owner_module("example")]
    mod example_owner {}
    #[domain_adapter_module("example")]
    mod example_adapter {}
    #[domain_port_module("example")]
    mod example_port {}
    #[stateright_model]
    struct ExampleModel;
    #[stateright_model]
    #[production_bound_stateright_model]
    struct ExampleProductionBoundModel;
    #[evidence_pending_foundation]
    struct ExampleEvidencePendingFoundation;
    #[move_only_resource]
    struct ExampleCapability;

    #[domain_resource_terminal]
    fn terminate_example_capability(_capability: ExampleCapability) {}

    #[domain_initial_owner_creation]
    fn create_example_owner() -> ExampleExclusiveOwner {
        ExampleExclusiveOwner
    }

    #[physical_observation_minter]
    fn mint_example_observation() -> ExampleObservation {
        ExampleObservation::Accepted
    }

    #[domain_invariant("example")]
    fn validate_example_state(_state: &ExampleCoreState) -> Result<(), ExampleFailure> {
        Ok(())
    }

    #[domain_error_mapper("example")]
    fn map_example_failure(_failure: std::io::Error) -> ExampleObservation {
        ExampleObservation::Accepted
    }

    fn assert_actor<T: DomainActorRole>() {}
    fn assert_event<T: DomainEventRole>() {}
    fn assert_exclusive_owner<T: ExclusiveDomainOwnerRole>() {}
    fn assert_ratcheted_owner<T: RatchetedOwnerRole>() {}
    fn assert_worker<T: PhysicalWorkerRole>() {}
    fn assert_adapter<T: ShippingAdapterRole>() {}
    fn assert_effect<T: DomainEffectRole>() {}
    fn assert_feedback<T: DomainFeedbackRole>() {}
    fn assert_failure<T: DomainFailureRole>() {}
    fn assert_rejection<T: DomainRejectionRole>() {}
    fn assert_result<T: ClosedDomainResultRole>() {}
    fn assert_observation<T: DomainObservationRole>() {}
    fn assert_projection<T: DomainProjectionRole>() {}
    fn assert_core_state<T: DomainCoreStateRole>() {}
    fn assert_model<T: StaterightModelRole>() {}
    fn assert_production_bound_model<T: ProductionBoundStaterightModelRole>() {}
    fn assert_evidence_pending_foundation<T: EvidencePendingFoundationRole>() {}
    fn assert_resource<T: MoveOnlyResourceRole>() {}

    #[test]
    fn attributes_emit_exact_role_identity_for_structs_enums_and_generics() {
        assert_actor::<ExampleOwner<u64>>();
        assert_event::<ExampleEvent>();
        assert_exclusive_owner::<ExampleExclusiveOwner>();
        assert_ratcheted_owner::<ExampleRatchetedOwner>();
        assert_worker::<ExampleWorker>();
        assert_adapter::<ExampleAdapter>();
        assert_effect::<ExampleEffect>();
        assert_feedback::<ExampleFeedback>();
        assert_failure::<ExampleFailure>();
        assert_rejection::<ExampleRejection>();
        let _rejection = ExampleRejection::NotAdmitted;
        assert_result::<ExampleResult>();
        assert_observation::<ExampleObservation>();
        assert_projection::<ExampleProjection>();
        assert_core_state::<ExampleCoreState>();
        fn assert_record<T: super::DomainRecordRole>() {}
        assert_record::<ExampleRecord<u64>>();
        assert_model::<ExampleModel>();
        assert_production_bound_model::<ExampleProductionBoundModel>();
        assert_evidence_pending_foundation::<ExampleEvidencePendingFoundation>();
        assert_resource::<ExampleCapability>();
        terminate_example_capability(ExampleCapability);
        let _ = create_example_owner();
        let _ = mint_example_observation();
        let _ = validate_example_state(&ExampleCoreState::Current);
        let _ = map_example_failure(std::io::Error::other("fixture"));
        let _ = ExampleEffect::Submit;
        let _ = ExampleEvent::Start;
        let _ = ExampleFeedback::Returned;
        let _ = ExampleFailure::Unavailable;
        let _ = ExampleResult::Complete;
        let _ = ExampleObservation::Accepted;
        let _ = ExampleProjection::Current;
        let _ = ExampleCoreState::Current;
    }
}
