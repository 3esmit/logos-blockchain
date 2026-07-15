use lb_cryptarchia_engine::Epoch;
use lb_key_management_system_keys::keys::{Ed25519Signature, ZkPublicKey, ZkSignature};

use super::{SDPDeclareOp, SdpError};
use crate::{
    events::Events,
    mantle::{
        Note, TxHash,
        ledger::{Declarations, Operation, Utxos},
    },
    sdp::{Declaration, MinStake, service_notes::ServiceNotes},
};

trait SDPDeclareValidationExt {
    fn validate(
        &self,
        note: Note,
        declarations: &Declarations,
        service_notes: &ServiceNotes,
        min_stake: &MinStake,
    ) -> Result<(), SdpError>;

    fn execute(
        &self,
        ctx: SDPDeclareExecutionContext,
    ) -> Result<(SDPDeclareExecutionContext, Events), SdpError>;
}

impl SDPDeclareValidationExt for SDPDeclareOp {
    fn validate(
        &self,
        note: Note,
        declarations: &Declarations,
        service_notes: &ServiceNotes,
        min_stake: &MinStake,
    ) -> Result<(), SdpError> {
        // Check that the declaration doesn't already exist
        if declarations.contains_key(&self.id()) {
            return Err(SdpError::DuplicateDeclaration(self.id()));
        }

        // Ensure value of service note is sufficient for joining the service.
        if note.value < min_stake.threshold {
            return Err(SdpError::NoteInsufficientValue {
                note_id: self.service_note_id,
                value: note.value,
            });
        }

        // Ensure the note has not already been locked for this service.
        if service_notes.is_locked_for_service(&self.service_note_id, &self.service_type) {
            return Err(SdpError::NoteAlreadyUsedForService {
                note_id: self.service_note_id,
                service_type: self.service_type,
            });
        }

        Ok(())
    }

    fn execute(
        &self,
        mut ctx: SDPDeclareExecutionContext,
    ) -> Result<(SDPDeclareExecutionContext, Events), SdpError> {
        let declaration_id = self.id();
        let declaration = Declaration::new(ctx.epoch, self);
        ctx.declarations = ctx.declarations.insert(declaration_id, declaration);
        let utxo = ctx
            .utxo_tree
            .utxos()
            .get(&self.service_note_id)
            .expect("The operation should have been checked")
            .0;

        ctx.service_notes = ctx
            .service_notes
            .lock(
                &ctx.min_stake,
                self.service_type,
                utxo.note(),
                &self.service_note_id,
            )
            .map_err(|_| SdpError::UnexpectedError)?;

        Ok((ctx, Events::new()))
    }
}

pub struct SDPDeclareValidationContext<'a> {
    pub utxo_tree: &'a Utxos,
    pub service_notes: &'a ServiceNotes,
    pub tx_hash: &'a TxHash,
    pub declare_zk_sig: &'a ZkSignature,
    pub declare_eddsa_sig: &'a Ed25519Signature,
    pub declarations: &'a Declarations,
    pub min_stake: &'a MinStake,
}

pub struct SDPDeclareGenesisValidationContext<'a> {
    pub utxo_tree: &'a Utxos,
    pub service_notes: &'a ServiceNotes,
    pub declarations: &'a Declarations,
    pub min_stake: &'a MinStake,
}

pub struct SDPDeclareExecutionContext {
    pub utxo_tree: Utxos,
    pub epoch: Epoch,
    pub declarations: Declarations,
    pub service_notes: ServiceNotes,
    pub min_stake: MinStake,
}

impl Operation<SDPDeclareValidationContext<'_>> for SDPDeclareOp {
    type ExecutionContext<'a>
        = SDPDeclareExecutionContext
    where
        Self: 'a;
    type Error = SdpError;

    fn validate(&self, ctx: &SDPDeclareValidationContext<'_>) -> Result<(), Self::Error> {
        // Check that the note exist
        let Some((utxo, _)) = ctx.utxo_tree.utxos().get(&self.service_note_id) else {
            return Err(SdpError::InexistingNote(self.service_note_id));
        };

        // Ensure service note exists and ownership over the service note and `zk_id`
        let note = utxo.note();
        if !ZkPublicKey::verify_multi(
            &[note.pk, self.zk_id],
            &ctx.tx_hash.to_fr(),
            ctx.declare_zk_sig,
        ) {
            return Err(SdpError::InvalidZkSignature);
        }

        // Ensure ownership over the `provider_id`
        self.provider_id
            .0
            .verify(
                ctx.tx_hash.as_signing_bytes().as_ref(),
                ctx.declare_eddsa_sig,
            )
            .map_err(|_| SdpError::InvalidEddsaSignature)?;

        SDPDeclareValidationExt::validate(
            self,
            note,
            ctx.declarations,
            ctx.service_notes,
            ctx.min_stake,
        )
    }

    fn execute(
        &self,
        ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
        SDPDeclareValidationExt::execute(self, ctx)
    }
}

impl Operation<SDPDeclareGenesisValidationContext<'_>> for SDPDeclareOp {
    type ExecutionContext<'a>
        = SDPDeclareExecutionContext
    where
        Self: 'a;
    type Error = SdpError;

    fn validate(&self, ctx: &SDPDeclareGenesisValidationContext<'_>) -> Result<(), Self::Error> {
        // Check that the note exist
        let Some((utxo, _)) = ctx.utxo_tree.utxos().get(&self.service_note_id) else {
            return Err(SdpError::InexistingNote(self.service_note_id));
        };
        let note = utxo.note();

        SDPDeclareValidationExt::validate(
            self,
            note,
            ctx.declarations,
            ctx.service_notes,
            ctx.min_stake,
        )
    }

    fn execute(
        &self,
        ctx: Self::ExecutionContext<'_>,
    ) -> Result<(Self::ExecutionContext<'_>, Events), Self::Error> {
        SDPDeclareValidationExt::execute(self, ctx)
    }
}
