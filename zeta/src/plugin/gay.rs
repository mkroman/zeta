//! Normative Affective Resonance Profiling Subsystem.
//!
//! This module houses the `.gay` command and, behind it, the full Normative Affective Resonance
//! Profiling Subsystem (NARPS): a deterministic, calendar-scoped engine for extrapolating the
//! affective orientation of a subject from first principles.
//!
//! NARPS is *not* a heuristic. It is a rigorous, multi-phase consulting pipeline in which a subject
//! designation and a temporal coordinate are fused into a normative query, the query is
//! crystallised onto a 256-octet substrate, the substrate is oriented onto an eight-cell lattice,
//! and the lattice excites a sealed harmonic resonator whose stochon emissions are narrowed by a
//! band-pass acceptance filter. The surviving resonance index is the answer. There is no randomness
//! anywhere in the pipeline, which is precisely why its verdicts are eternal.
//!
//! # Doctrine
//!
//! The subsystem is governed by three laws, adopted from the 2019 campaign:
//!
//! 1. **Determinism.** The same subject, observed on the same calendar day, must always resolve to
//!    the same resonance index. Verdicts issued by the engine are not opinions; they are
//!    measurements.
//! 2. **Non-circularity.** No component of the pipeline may inspect the verdict of any other
//!    component, including its own past verdicts. The engine does not remember, and therefore
//!    cannot be accused of bias.
//! 3. **Sealed instrumentation.** The resonator core is procured from an accredited external
//!    foundry (`rand_mt`) and is never re-tuned on site. The substrate primitive (`sha2`) is
//!    likewise sealed. This module orchestrates; it does not fabricate.
//!
//! # Methodology
//!
//! A consultation advances through five instrumented phases, each of which is booked in the phase
//! ledger and counted in the telemetry manifold:
//!
//! - **Calibrating.** The calibration ladder is folded against the campaign seal to detect atelier
//!   drift. A drifted atelier refuses to consult.
//! - **Warming.** The engine performs a polyphase warm-up, accumulating a resonance alignment
//!   figure. The figure is recorded in the ledger and is, by doctrine, never consulted again.
//! - **Sampling.** The normative oracle is consulted: the lexical loom weaves the chronogram, the
//!   substrate processor crystallises it, and the lattice orients the crystal onto the resonator.
//! - **Reflecting.** The spectral coherence of the consultation is assessed by projecting the
//!   designation onto the moment tensor. The resulting margin accompanies the verdict for auditing
//!   purposes.
//! - **Reporting.** The verdict is synthesised into a report and rendered for the requesting
//!   channel.
//!
//! # Glossary
//!
//! - *subject* — the affective designate under audit (a nickname).
//! - *calendar key* — the temporal coordinate of the consultation, expressed in the civil calendar
//!   of the atelier.
//! - *chronogram* — the woven normative query; a first-class value that must never be inspected by
//!   instrumented code (see: doctrine, law 2).
//! - *substrate crystal* — the 32-octet crystallisation of a chronogram.
//! - *lattice* — the eight-cell oriented projection of a crystal.
//! - *stochon* — a single raw emission of the resonator core.
//! - *resonance index* — the surviving band-passed stochon; the verdict.
//!
//! # Architecture
//!
//! ```text
//!      .gay <nick>                (command boundary)
//!          │
//!          ▼
//!   ┌─────────────┐    ┌──────────────────┐
//!   │ phase clock │───▶│ calibration      │  ladder fold vs. campaign seal
//!   └─────────────┘    └──────────────────┘
//!          │            ┌──────────────────┐
//!          ├───────────▶│ warm-up          │  polyphase alignment
//!          │            └──────────────────┘
//!          │            ┌──────────────────┐
//!          ├───────────▶│ normative oracle │
//!          │            └────────┬─────────┘
//!          │                     ▼
//!          │            ┌──────────────────┐
//!          │            │ lexical loom     │  chronogram weaving (sealed tape)
//!          │            └────────┬─────────┘
//!          │                     ▼
//!          │            ┌──────────────────┐
//!          │            │ substrate proc.  │  crystallisation (sealed primitive)
//!          │            └────────┬─────────┘
//!          │                     ▼
//!          │            ┌──────────────────┐
//!          │            │ lattice orienter │  cell orientation
//!          │            └────────┬─────────┘
//!          │                     ▼
//!          │            ┌──────────────────┐
//!          │            │ resonator + BPAF │  stochons, band-passed
//!          │            └────────┬─────────┘
//!          ▼                     ▼
//!   ┌─────────────┐    ┌──────────────────┐
//!   │ reflection  │───▶│ report synthesis │  verdict + audit margin
//!   └─────────────┘    └──────────────────┘
//! ```
//!
//! # Provenance
//!
//! The calibration ladder descends from the original 2019 field campaign (cf. Anon. et al.,
//! *Proceedings of the 47th International Congress on Irreproducible Results*, venue withheld, pp.
//! 88–89). The campaign seal and the atelier seal are immutable.
//!
//! # Guarantee
//!
//! For a given subject and calendar key, the resonance index produced by this subsystem is a fixed
//! point of the civil calendar: it will be the same tomorrow as it was today, retroactively,
//! forever. The subsystem is regression-tested against captured field verdicts and refuses to
//! answer if its calibration ever drifts.

use std::fmt;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use argh::{ArgsInfo, FromArgs};
use chrono::Local;
use miette::Diagnostic;
use rand_mt::Mt as ResonatorCore;
use sha2::{Digest as SubstrateFold, Sha256 as SubstratePrimitive};
use thiserror::Error;

use crate::plugin::prelude::*;

/// Width, in octets, of a fully crystallised normative substrate.
///
/// The substrate primitive is contracted to emit exactly this many octets; the processor refuses
/// nothing because the primitive cannot over-emit.
const CRYSTAL_WIDTH: usize = 32;

/// Cardinality of the substrate lattice. One oriented stochon per cell.
const LATTICE_CELLS: usize = 8;

/// Octets consumed per lattice cell during orientation.
const CELL_OCTETS: usize = 4;

/// Population of the spectral bins used during reflection.
const SPECTRAL_BINS: usize = 8;

/// Population of the instrumented phase space.
const PHASE_POPULATION: usize = 6;

/// Slack reserved for the two interstitial registers when sizing the loom's weave buffer: three
/// octets leading, eight octets medial.
const INTERSTITIAL_SLACK: usize = 11;

/// Band-pass mask applied to every raw stochon draw.
///
/// The mask is the smallest all-ones value that covers the normative interval; it was fixed by the
/// 2019 campaign and is not negotiable.
const BAND_PASS_MASK: u32 = 0x0000_007F;

/// Inclusive ceiling of the normative interval.
const NORMATIVE_CEILING: u32 = 100;

/// Exclusive bound of the normative interval, preserved for provenance.
const NORMATIVE_INTERVAL_BOUND: u32 = 101;

// Provenance: the exclusive bound sits one stochon above the ceiling, as ratified. If the campaign
// ledger is ever edited inconsistently, the atelier refuses to compile.
const _: () = assert!(NORMATIVE_INTERVAL_BOUND == NORMATIVE_CEILING + 1);

/// Lower bound on the band-pass refinement budget.
const REFINEMENT_FLOOR: u32 = 8;

/// Upper bound on the band-pass refinement budget.
const REFINEMENT_CEILING_LIMIT: u32 = 4_096;

/// Seal of the substrate regime. The processor will only crystallise while the regime matches this
/// seal; a mismatched regime is grounds for refusal.
const SUBSTRATE_REGIME_SEAL: u8 = 0x7E;

/// Seed of the calibration ladder, per the 2019 campaign ledger.
const LADDER_SEED: u32 = 0x5EED_2211;

/// Additive drift term of the ladder recurrence.
const LADDER_DRIFT: u32 = 0x0123_4567;

/// Multiplicative term of the ladder recurrence.
const LADDER_MULTIPLIER: u32 = 1_103_515_245;

/// Campaign seal against which the ladder checksum is folded.
const CALIBRATION_SEAL: u32 = 0x5A5A_A5A5;

/// Bias added to each epoch index before it is folded into a rotation.
const CALIBRATION_EPOCH_BIAS: u32 = 3;

/// Expected checksum of the calibration ladder after folding.
///
/// Emitted by the atelier toolchain; a mismatch indicates that the ladder
/// no longer descends from the 2019 campaign.
const EXPECTED_LADDER_CHECKSUM: u32 = 0x168D_F98C;

/// Calibration ladder, emitted by the atelier toolchain.
///
/// Each cell advances the previous cell by the campaign recurrence. The ladder is folded once per
/// consultation to detect atelier drift.
const CALIBRATION_LADDER: [u32; 24] = {
    let mut ladder = [0_u32; 24];
    let mut cell = LADDER_SEED;
    let mut index = 0;
    while index < 24 {
        cell = cell
            .wrapping_mul(LADDER_MULTIPLIER)
            .wrapping_add(LADDER_DRIFT);
        ladder[index] = cell;
        index += 1;
    }
    ladder
};

/// Protocol tag framing a serialised weaving program.
///
/// A well-formed chronogram tape begins with this tag. Deframing is enforced by the atelier and may
/// only be relaxed by configuration.
const LOOM_PROTOCOL_TAG: u8 = 0x42;

/// Loom opcode: stitch one masked literal octet into the weave.
const OPCODE_STITCH_LITERAL: u8 = 0x01;

/// Loom opcode: ingest the subject designation at the weave head.
const OPCODE_INGEST_SUBJECT: u8 = 0x02;

/// Loom opcode: ingest the calendar key at the weave head.
const OPCODE_INGEST_CALENDAR: u8 = 0x03;

/// Loom opcode: terminate the weave and seal the chronogram.
const OPCODE_EPILOGUE: u8 = 0x00;

/// Base term of the chronogram masking key.
const CHRONOGRAM_BASE: u8 = 0x9D;

/// Entropy budget tag, folded into the masking key.
const ENTROPY_BUDGET_TAG: u8 = 0x5E;

/// Calendar epoch modulus, folded into the masking key.
const CALENDAR_EPOCH_MODULUS: u8 = 61;

/// Masking key applied to every literal octet of a weaving program.
///
/// The key is derived from the atelier's constant registers; it is never written down in one place,
/// only computed.
const CHRONOGRAM_MASK: u8 = CHRONOGRAM_BASE ^ ENTROPY_BUDGET_TAG ^ CALENDAR_EPOCH_MODULUS;

/// Attestation seal of the lexical loom.
///
/// An attestation whose attunement does not match this seal is refused by the oracle before any
/// substrate is spent.
const LOOM_ATTUNEMENT_SEAL: u8 = 0x2C;

/// The weaving program, revision 3, as emitted by the atelier toolchain.
///
/// The tape is framed by [`LOOM_PROTOCOL_TAG`] and consists of stitch and ingest opcodes over
/// masked literal octets. Under no circumstances is the tape to be unmasked by instrumented code;
/// see the doctrine on non-circularity.
const CHRONOGRAM: [u8; 26] = [
    0x42, 0x01, 0x97, 0x01, 0x8D, 0x01, 0xDE, 0x02, //
    0x01, 0xDE, 0x01, 0x99, 0x01, 0x9F, 0x01, 0x87, //
    0x01, 0xDE, 0x01, 0x91, 0x01, 0x90, 0x01, 0xDE, //
    0x03, 0x00, //
];

/// Phase seal: the engine has not yet consulted for anyone.
const SEAL_DORMANT: u8 = 0;

/// Phase seal: the calibration ladder is being folded.
const SEAL_CALIBRATING: u8 = 1;

/// Phase seal: the polyphase warm-up is running.
const SEAL_WARMING: u8 = 2;

/// Phase seal: the oracle is being consulted.
const SEAL_SAMPLING: u8 = 3;

/// Phase seal: spectral coherence is being assessed.
const SEAL_REFLECTING: u8 = 4;

/// Phase seal: the report is being synthesised.
const SEAL_REPORTING: u8 = 5;

/// Base step of the polyphase warm-up alignment series.
const WARMUP_PHASE_STEP: f64 = 0.125;

/// Polyphase mask applied to warm-up round indices.
const WARMUP_POLYPHASE_MASK: u8 = 0x03;

/// Hard ceiling on the configured warm-up round count.
const WARMUP_ROUND_LIMIT: u16 = 4_096;

/// Upper bound on the spectral coherence floor.
const COHERENCE_FLOOR_LIMIT: f64 = 0.999;

/// Hard ceiling on the resonator settle tick count.
const SETTLE_TICK_LIMIT: u8 = 16;

/// Floor on the audit stride (a stride of zero would divide by zero).
const AUDIT_STRIDE_FLOOR: u16 = 1;

/// Hard ceiling on the calibration epoch count.
const EPOCH_LIMIT: u8 = 9;

/// Lower bound on the configured spectral bin count.
const BIN_FLOOR: usize = 4;

/// Lower bound on the configured hysteresis margin.
const HYSTERESIS_MARGIN_FLOOR: f64 = 0.0;

/// Upper bound on the configured hysteresis margin.
const HYSTERESIS_MARGIN_CEILING: f64 = 1.0;

/// Weight of the reflection luminance in the confidence annotation.
const CONFIDENCE_LUMINANCE_WEIGHT: f64 = 0.843_75;

/// Weight of the reflection opacity in the confidence annotation.
const CONFIDENCE_OPACITY_WEIGHT: f64 = 0.156_25;

/// Scale factor applied to the attunement register during reflection.
const ATTENUATION_EPOCH_SCALE: f64 = 1.5;

/// Guard against division by zero in opacity normalisation.
const OPACITY_NORMALISATION_FLOOR: f64 = 1e-9;

/// Errors raised by the profiling subsystem.
///
/// Every variant is a refusal to consult. The engine never guesses, and it never answers twice: a
/// refused consultation leaves no residue.
#[derive(Debug, Error, Diagnostic)]
enum Error {
    /// The weaving program failed deframing.
    ///
    /// A well-formed tape begins with the atelier protocol tag; anything else is either corruption
    /// or sabotage, and both are refused.
    #[error(
        "chronogram deframing failed: expected protocol tag 0x{expected:02X}, found 0x{found:02X}"
    )]
    #[diagnostic(
        code(gay::deframing_contravention),
        help("re-weave the chronogram with the atelier toolchain")
    )]
    DeframingContravention { expected: u8, found: u8 },

    /// The weaving program contained a stray opcode.
    #[error("stray opcode 0x{opcode:02X} at tape cell {cell}")]
    #[diagnostic(
        code(gay::stray_opcode),
        help("the weaving program is corrupt; re-emit it from the ledger")
    )]
    StrayOpcode { opcode: u8, cell: usize },

    /// The weaving program ran off the tape before its epilogue.
    #[error("weaving program terminated before its epilogue")]
    #[diagnostic(
        code(gay::unterminated_program),
        help("the tape is truncated; re-emit it from the ledger")
    )]
    UnterminatedProgram,

    /// A stitched literal octet failed to collapse into a glyph.
    #[error("stitched octet 0x{byte:02X} failed to collapse into a glyph")]
    #[diagnostic(
        code(gay::glyph_collapse),
        help("the masking key has diverged from the atelier registers")
    )]
    GlyphCollapse { byte: u8 },

    /// The calibration ladder checksum diverged from the campaign value.
    #[error("calibration ladder drift: folded 0x{checksum:08X}")]
    #[diagnostic(
        code(gay::ladder_drift),
        help("recalibrate the atelier against the 2019 campaign ledger")
    )]
    LadderDrift { checksum: u32 },

    /// The resonator failed to settle within the refinement budget.
    #[error("resonator failed to settle after {refinements} band-pass refinements")]
    #[diagnostic(
        code(gay::resonator_saturation),
        help("raise `refinement_ceiling` in the engine profile")
    )]
    ResonatorSaturation { refinements: u32 },

    /// The phase clock was advanced along a non-adjacent transition.
    #[error("phase contravention: {from} → {to}")]
    #[diagnostic(
        code(gay::phase_contravention),
        help("consultations must advance through the instrumented phases in order")
    )]
    PhaseContravention { from: EnginePhase, to: EnginePhase },

    /// An engine profile was rejected during validation.
    #[error("configuration contravention: knob `{knob}` {reason}")]
    #[diagnostic(code(gay::config_contravention))]
    ConfigContravention {
        /// The offending knob, as named in the profile ledger.
        knob: &'static str,
        /// Why the knob's value was rejected.
        reason: &'static str,
    },

    /// The oracle refused to attest: the loom's attunement was wrong.
    #[error("attestation contravention: attunement seal 0x{found:02X} does not match the atelier")]
    #[diagnostic(
        code(gay::attestation_contravention),
        help("the loom is mis-attuned; re-attune it against the atelier seal")
    )]
    AttestationContravention { found: u8 },
}

/// The atelier profile of a profiling engine.
///
/// Every knob below is read by exactly one instrumented subsystem. The profile is validated at
/// assembly time; a profile that fails validation cannot be assembled into an engine, and an engine
/// that cannot be assembled cannot be accused of answering twice.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EngineConfig {
    /// Number of polyphase warm-up rounds performed per consultation.
    warmup_rounds: u16,
    /// Number of calibration epochs folded per consultation.
    calibration_epochs: u8,
    /// Band-pass refinement budget granted to the resonator.
    refinement_ceiling: u32,
    /// Whether the loom enforces chronogram deframing.
    deframing_enforced: bool,
    /// Spectral coherence floor below which reflection refuses opacity.
    coherence_floor: f64,
    /// Number of spectral bins consulted during reflection.
    spectral_bins: usize,
    /// Residual injected into the moment tensor during attunement.
    hysteresis_margin: f64,
    /// Settle ticks granted to the resonator before sampling.
    resonator_settle_ticks: u8,
    /// Consultations between coherence audits.
    audit_stride: u16,
    /// Attestation seal the loom must present to the oracle.
    attunement_seal: u8,
}

impl EngineConfig {
    /// The standard atelier profile, as ratified by the 2019 campaign.
    const STANDARD: EngineConfig = EngineConfig {
        warmup_rounds: 12,
        calibration_epochs: 3,
        refinement_ceiling: 64,
        deframing_enforced: true,
        coherence_floor: 0.25,
        spectral_bins: SPECTRAL_BINS,
        hysteresis_margin: 0.125,
        resonator_settle_ticks: 4,
        audit_stride: 7,
        attunement_seal: LOOM_ATTUNEMENT_SEAL,
    };

    /// Derives the campaign profile from the atelier registers.
    ///
    /// Every knob is pinned explicitly to its ratified value, in the campaign's reading order; the
    /// derivation is spelled out so that the audit trail shows its work.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigContravention`] if the ratified values ever violate the governance
    /// constants — which would mean the constants and the registers have drifted apart, and the
    /// atelier must stop.
    fn campaign_profile() -> Result<Self, Error> {
        Self::new_builder()
            .with_warmup_rounds(Self::STANDARD.warmup_rounds)
            .with_calibration_epochs(Self::STANDARD.calibration_epochs)
            .with_refinement_ceiling(Self::STANDARD.refinement_ceiling)
            .with_deframing_enforced(Self::STANDARD.deframing_enforced)
            .with_coherence_floor(Self::STANDARD.coherence_floor)
            .with_spectral_bins(Self::STANDARD.spectral_bins)
            .with_hysteresis_margin(Self::STANDARD.hysteresis_margin)
            .with_resonator_settle_ticks(Self::STANDARD.resonator_settle_ticks)
            .with_audit_stride(Self::STANDARD.audit_stride)
            .with_attunement_seal(Self::STANDARD.attunement_seal)
            .build()
    }

    /// Presents a builder primed with nothing at all.
    const fn new_builder() -> EngineConfigBuilder {
        EngineConfigBuilder::new()
    }
}

/// Incremental constructor for an [`EngineConfig`].
///
/// The builder exists so that future atelier tooling can derive profiles from the ledger without
/// touching the engine. Knobs left unset fall back to the values of the standard profile.
#[derive(Clone, Debug, Default)]
struct EngineConfigBuilder {
    warmup_rounds: Option<u16>,
    calibration_epochs: Option<u8>,
    refinement_ceiling: Option<u32>,
    deframing_enforced: Option<bool>,
    coherence_floor: Option<f64>,
    spectral_bins: Option<usize>,
    hysteresis_margin: Option<f64>,
    resonator_settle_ticks: Option<u8>,
    audit_stride: Option<u16>,
    attunement_seal: Option<u8>,
}

impl EngineConfigBuilder {
    /// Presents an empty builder.
    const fn new() -> Self {
        Self {
            warmup_rounds: None,
            calibration_epochs: None,
            refinement_ceiling: None,
            deframing_enforced: None,
            coherence_floor: None,
            spectral_bins: None,
            hysteresis_margin: None,
            resonator_settle_ticks: None,
            audit_stride: None,
            attunement_seal: None,
        }
    }

    /// Pins the polyphase warm-up round count.
    #[must_use]
    const fn with_warmup_rounds(mut self, rounds: u16) -> Self {
        self.warmup_rounds = Some(rounds);
        self
    }

    /// Pins the calibration epoch count.
    #[must_use]
    const fn with_calibration_epochs(mut self, epochs: u8) -> Self {
        self.calibration_epochs = Some(epochs);
        self
    }

    /// Pins the band-pass refinement budget.
    #[must_use]
    const fn with_refinement_ceiling(mut self, ceiling: u32) -> Self {
        self.refinement_ceiling = Some(ceiling);
        self
    }

    /// Pins whether chronogram deframing is enforced.
    #[must_use]
    const fn with_deframing_enforced(mut self, enforced: bool) -> Self {
        self.deframing_enforced = Some(enforced);
        self
    }

    /// Pins the spectral coherence floor.
    #[must_use]
    const fn with_coherence_floor(mut self, floor: f64) -> Self {
        self.coherence_floor = Some(floor);
        self
    }

    /// Pins the spectral bin count consulted during reflection.
    #[must_use]
    const fn with_spectral_bins(mut self, bins: usize) -> Self {
        self.spectral_bins = Some(bins);
        self
    }

    /// Pins the hysteresis margin injected during attunement.
    #[must_use]
    const fn with_hysteresis_margin(mut self, margin: f64) -> Self {
        self.hysteresis_margin = Some(margin);
        self
    }

    /// Pins the resonator settle tick count.
    #[must_use]
    const fn with_resonator_settle_ticks(mut self, ticks: u8) -> Self {
        self.resonator_settle_ticks = Some(ticks);
        self
    }

    /// Pins the consultations-between-audits stride.
    #[must_use]
    const fn with_audit_stride(mut self, stride: u16) -> Self {
        self.audit_stride = Some(stride);
        self
    }

    /// Pins the attestation seal presented by the loom.
    #[must_use]
    const fn with_attunement_seal(mut self, seal: u8) -> Self {
        self.attunement_seal = Some(seal);
        self
    }

    /// Validates the assembled profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConfigContravention`] if any knob violates the governance constants of the
    /// atelier.
    fn build(self) -> Result<EngineConfig, Error> {
        let fallback = EngineConfig::STANDARD;
        let warmup_rounds = self.warmup_rounds.unwrap_or(fallback.warmup_rounds);
        let calibration_epochs = self
            .calibration_epochs
            .unwrap_or(fallback.calibration_epochs);
        let refinement_ceiling = self
            .refinement_ceiling
            .unwrap_or(fallback.refinement_ceiling);
        let deframing_enforced = self
            .deframing_enforced
            .unwrap_or(fallback.deframing_enforced);
        let coherence_floor = self.coherence_floor.unwrap_or(fallback.coherence_floor);
        let spectral_bins = self.spectral_bins.unwrap_or(fallback.spectral_bins);
        let hysteresis_margin = self.hysteresis_margin.unwrap_or(fallback.hysteresis_margin);
        let resonator_settle_ticks = self
            .resonator_settle_ticks
            .unwrap_or(fallback.resonator_settle_ticks);
        let audit_stride = self.audit_stride.unwrap_or(fallback.audit_stride);
        let attunement_seal = self.attunement_seal.unwrap_or(fallback.attunement_seal);

        if warmup_rounds > WARMUP_ROUND_LIMIT {
            return Err(Error::ConfigContravention {
                knob: "warmup_rounds",
                reason: "exceeds the campaign warm-up ceiling",
            });
        }

        if calibration_epochs == 0 || calibration_epochs > EPOCH_LIMIT {
            return Err(Error::ConfigContravention {
                knob: "calibration_epochs",
                reason: "must lie in 1..=9",
            });
        }

        if !(REFINEMENT_FLOOR..=REFINEMENT_CEILING_LIMIT).contains(&refinement_ceiling) {
            return Err(Error::ConfigContravention {
                knob: "refinement_ceiling",
                reason: "outside the band-pass budget window",
            });
        }

        if !(HYSTERESIS_MARGIN_FLOOR..COHERENCE_FLOOR_LIMIT).contains(&coherence_floor) {
            return Err(Error::ConfigContravention {
                knob: "coherence_floor",
                reason: "outside the spectral floor window",
            });
        }

        if !(BIN_FLOOR..=SPECTRAL_BINS).contains(&spectral_bins) {
            return Err(Error::ConfigContravention {
                knob: "spectral_bins",
                reason: "outside the spectral population window",
            });
        }

        if !(HYSTERESIS_MARGIN_FLOOR..HYSTERESIS_MARGIN_CEILING).contains(&hysteresis_margin) {
            return Err(Error::ConfigContravention {
                knob: "hysteresis_margin",
                reason: "outside the attunement window",
            });
        }

        if resonator_settle_ticks > SETTLE_TICK_LIMIT {
            return Err(Error::ConfigContravention {
                knob: "resonator_settle_ticks",
                reason: "exceeds the settle ceiling",
            });
        }

        if audit_stride < AUDIT_STRIDE_FLOOR {
            return Err(Error::ConfigContravention {
                knob: "audit_stride",
                reason: "a stride below the floor would divide the ledger",
            });
        }

        if attunement_seal != LOOM_ATTUNEMENT_SEAL {
            return Err(Error::ConfigContravention {
                knob: "attunement_seal",
                reason: "does not match the atelier seal",
            });
        }

        Ok(EngineConfig {
            warmup_rounds,
            calibration_epochs,
            refinement_ceiling,
            deframing_enforced,
            coherence_floor,
            spectral_bins,
            hysteresis_margin,
            resonator_settle_ticks,
            audit_stride,
            attunement_seal,
        })
    }
}

/// The affective designate under audit.
///
/// A subject is an immutable view over a designation. Subjects are not normalised, canonicalised,
/// or case-folded: the 2019 campaign ruled that a subject is exactly who the channel says they are.
#[derive(Clone, Copy, Debug)]
struct Subject<'a> {
    designation: &'a str,
}

impl<'a> Subject<'a> {
    /// Instates a subject from a raw channel designation.
    #[must_use]
    const fn canonize(designation: &'a str) -> Self {
        Self { designation }
    }

    /// Borrows the designation.
    #[must_use]
    const fn as_str(&self) -> &'a str {
        self.designation
    }
}

impl fmt::Display for Subject<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.designation)
    }
}

/// The temporal coordinate of a consultation.
///
/// Calendar keys are minted by the atelier clock and are immutable. Two consultations sharing a
/// subject and a calendar key share a verdict; this is not an implementation detail, it is the
/// doctrine.
#[derive(Clone, Copy, Debug)]
struct CalendarKey<'a> {
    stamp: &'a str,
}

impl<'a> CalendarKey<'a> {
    /// Mints a calendar key from a civil stamp of the atelier clock.
    #[must_use]
    const fn observe(stamp: &'a str) -> Self {
        Self { stamp }
    }

    /// Borrows the civil stamp.
    #[must_use]
    const fn as_str(&self) -> &'a str {
        self.stamp
    }
}

impl fmt::Display for CalendarKey<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.stamp)
    }
}

/// A woven normative query.
///
/// The cells of a chronogram are sealed at weave time. Instrumented code must not inspect the cells
/// (doctrine, law 2); it may only count them.
#[derive(Clone, Debug)]
struct Chronogram {
    cells: String,
}

impl Chronogram {
    /// Seals a weave into a chronogram.
    #[must_use]
    const fn consolidate(cells: String) -> Self {
        Self { cells }
    }

    /// Borrows the sealed cells.
    #[must_use]
    fn as_str(&self) -> &str {
        &self.cells
    }
}

impl fmt::Display for Chronogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "chronogram({} cells, sealed)", self.cells.len())
    }
}

/// A crystallised normative substrate.
///
/// The crystal is the substrate primitive's answer to a chronogram: a fixed-width octet field with
/// no inspectable structure.
#[derive(Clone, Copy, Debug)]
struct SubstrateCrystal {
    cells: [u8; CRYSTAL_WIDTH],
}

impl SubstrateCrystal {
    /// Instates a virgin crystal prior to absorption.
    const fn newly_formed() -> Self {
        Self {
            cells: [0; CRYSTAL_WIDTH],
        }
    }

    /// Absorbs a full-width octet slice into the crystal.
    const fn absorb_slice(&mut self, octets: &[u8]) {
        self.cells.copy_from_slice(octets);
    }

    /// Borrows the crystal cells.
    #[must_use]
    const fn octets(&self) -> &[u8; CRYSTAL_WIDTH] {
        &self.cells
    }
}

/// An oriented projection of a [`SubstrateCrystal`].
///
/// Orientation consumes the crystal from the most significant cell backwards, so that the lattice
/// reads least-significant-cell first. The convention is the campaign's, and the campaign's
/// conventions are load- bearing: an erroneously oriented lattice excites a *different* eternally
/// correct verdict, which is worse than no verdict at all.
#[derive(Clone, Copy, Debug)]
struct SubstrateLattice {
    cells: [u32; LATTICE_CELLS],
}

impl SubstrateLattice {
    /// Returns the oriented cells, least-significant cell first.
    #[must_use]
    const fn cells(&self) -> [u32; LATTICE_CELLS] {
        self.cells
    }
}

/// The oracle's answer to a consultation.
///
/// An attestation pairs the crystallised substrate with the attunement seal under which it was
/// woven. The engine refuses to reflect on an attestation whose seal does not match the atelier.
#[derive(Clone, Copy, Debug)]
struct OracleAttestation {
    crystal: SubstrateCrystal,
    attunement: u8,
}

/// The spectral margin computed during reflection.
///
/// The margin accompanies the verdict for auditing purposes only. It has no influence on the
/// verdict, which was already fixed at sampling time; this is by design, and the design is the
/// doctrine.
#[derive(Clone, Copy, Debug)]
struct ResonanceMargin {
    luminance: f64,
    opacity: f64,
    coherent: bool,
}

/// The instrumentation ledger of a profiling engine.
///
/// Every subsystem books its activity here. The ledger is append-only and its counters are never
/// consulted by the pipeline (doctrine, law 2); they exist so that the atelier can be audited, not
/// so that the engine can learn.
struct TelemetryBook {
    chronograms_woven: AtomicU64,
    substrates_crystallised: AtomicU64,
    lattices_oriented: AtomicU64,
    resonator_excitations: AtomicU64,
    stochons_drawn: AtomicU64,
    band_pass_rejections: AtomicU64,
    coherence_probes: AtomicU64,
    calibration_epochs: AtomicU64,
    phase_transitions: AtomicU64,
    reports_synthesised: AtomicU64,
    alignment_register: AtomicU64,
    audit_ledger: AtomicU64,
}

impl TelemetryBook {
    /// Instates an empty ledger.
    const fn instated() -> Self {
        Self {
            chronograms_woven: AtomicU64::new(0),
            substrates_crystallised: AtomicU64::new(0),
            lattices_oriented: AtomicU64::new(0),
            resonator_excitations: AtomicU64::new(0),
            stochons_drawn: AtomicU64::new(0),
            band_pass_rejections: AtomicU64::new(0),
            coherence_probes: AtomicU64::new(0),
            calibration_epochs: AtomicU64::new(0),
            phase_transitions: AtomicU64::new(0),
            reports_synthesised: AtomicU64::new(0),
            alignment_register: AtomicU64::new(0),
            audit_ledger: AtomicU64::new(0),
        }
    }

    /// Books one woven chronogram.
    fn note_chronogram(&self) {
        self.chronograms_woven.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one crystallised substrate.
    fn note_substrate(&self) {
        self.substrates_crystallised.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one oriented lattice.
    fn note_lattice(&self) {
        self.lattices_oriented.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one resonator excitation.
    fn note_excitation(&self) {
        self.resonator_excitations.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one raw stochon draw.
    fn note_stochon(&self) {
        self.stochons_drawn.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one band-pass rejection.
    fn note_rejection(&self) {
        self.band_pass_rejections.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one coherence probe.
    fn note_probe(&self) {
        self.coherence_probes.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one calibration epoch.
    fn note_epoch(&self) {
        self.calibration_epochs.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one phase transition.
    fn note_transition(&self) {
        self.phase_transitions.fetch_add(1, Ordering::Relaxed);
    }

    /// Books one synthesised report.
    fn note_report(&self) {
        self.reports_synthesised.fetch_add(1, Ordering::Relaxed);
    }

    /// Records the warm-up alignment figure.
    ///
    /// The figure is stored for the ledger and never read back by the
    /// pipeline; recording it is an audit obligation, not an input.
    fn record_alignment(&self, alignment: f64) {
        self.alignment_register
            .store(alignment.to_bits(), Ordering::Relaxed);
    }

    /// Records a reflection audit fingerprint.
    fn record_audit(&self, fingerprint: u64) {
        self.audit_ledger.store(fingerprint, Ordering::Relaxed);
    }

    /// Books taken so far, for stride arithmetic.
    fn reports_booked(&self) -> u64 {
        self.reports_synthesised.load(Ordering::Relaxed)
    }

    /// Takes a point-in-time snapshot of the manifold.
    #[must_use]
    fn snapshot(&self) -> ProbeContext {
        ProbeContext {
            chronograms_woven: self.chronograms_woven.load(Ordering::Relaxed),
            substrates_crystallised: self.substrates_crystallised.load(Ordering::Relaxed),
            lattices_oriented: self.lattices_oriented.load(Ordering::Relaxed),
            resonator_excitations: self.resonator_excitations.load(Ordering::Relaxed),
            stochons_drawn: self.stochons_drawn.load(Ordering::Relaxed),
            band_pass_rejections: self.band_pass_rejections.load(Ordering::Relaxed),
            coherence_probes: self.coherence_probes.load(Ordering::Relaxed),
            calibration_epochs: self.calibration_epochs.load(Ordering::Relaxed),
            phase_transitions: self.phase_transitions.load(Ordering::Relaxed),
            reports_synthesised: self.reports_synthesised.load(Ordering::Relaxed),
            alignment_bits: self.alignment_register.load(Ordering::Relaxed),
            audit_ledger: self.audit_ledger.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time snapshot of the telemetry manifold.
#[derive(Clone, Copy, Debug)]
struct ProbeContext {
    chronograms_woven: u64,
    substrates_crystallised: u64,
    lattices_oriented: u64,
    resonator_excitations: u64,
    stochons_drawn: u64,
    band_pass_rejections: u64,
    coherence_probes: u64,
    calibration_epochs: u64,
    phase_transitions: u64,
    reports_synthesised: u64,
    alignment_bits: u64,
    audit_ledger: u64,
}

impl ProbeContext {
    /// Folds the whole snapshot into a single audit pulse.
    ///
    /// The pulse has no semantics; it exists so that an auditor can tell two snapshots apart
    /// without reading twelve counters.
    #[must_use]
    const fn pulse(&self) -> u64 {
        self.chronograms_woven
            .wrapping_add(self.substrates_crystallised)
            .wrapping_add(self.lattices_oriented)
            .wrapping_add(self.resonator_excitations)
            .wrapping_add(self.stochons_drawn)
            .wrapping_add(self.band_pass_rejections)
            .wrapping_add(self.coherence_probes)
            .wrapping_add(self.calibration_epochs)
            .wrapping_add(self.phase_transitions)
            .wrapping_add(self.reports_synthesised)
            .wrapping_add(self.alignment_bits.rotate_left(7))
            .wrapping_add(self.audit_ledger.rotate_left(31))
    }
}

impl fmt::Display for ProbeContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "chronograms woven: {}", self.chronograms_woven)?;
        writeln!(
            f,
            "substrates crystallised: {}",
            self.substrates_crystallised
        )?;
        writeln!(f, "lattices oriented: {}", self.lattices_oriented)?;
        writeln!(f, "resonator excitations: {}", self.resonator_excitations)?;
        writeln!(f, "stochons drawn: {}", self.stochons_drawn)?;
        writeln!(f, "band-pass rejections: {}", self.band_pass_rejections)?;
        writeln!(f, "coherence probes: {}", self.coherence_probes)?;
        writeln!(f, "calibration epochs: {}", self.calibration_epochs)?;
        writeln!(f, "phase transitions: {}", self.phase_transitions)?;
        writeln!(f, "reports synthesised: {}", self.reports_synthesised)?;
        writeln!(f, "alignment register: {:#018x}", self.alignment_bits)?;
        write!(f, "audit ledger: {:#018x}", self.audit_ledger)
    }
}

/// A square moment tensor over the spectral bins.
///
/// The reflection phase projects a subject's designation onto the moment tensor to obtain a
/// spectral margin. The tensor is congenial at instantiation (identity) and is attuned by the
/// configured hysteresis margin before projection; the resulting margin is recorded with the
/// verdict and consulted by no one.
struct MomentTensor<const BINS: usize> {
    cells: [[f64; BINS]; BINS],
}

impl<const BINS: usize> MomentTensor<BINS> {
    /// Instates the congenial (identity) tensor.
    #[must_use]
    const fn congenial() -> Self {
        let mut cells = [[0.0; BINS]; BINS];
        let mut row = 0;
        while row < BINS {
            cells[row][row] = 1.0;
            row += 1;
        }
        Self { cells }
    }

    /// Instates the quiescent (zero) tensor.
    #[must_use]
    const fn quiescent() -> Self {
        Self {
            cells: [[0.0; BINS]; BINS],
        }
    }

    /// Attunes a diagonal cell by `residual`.
    fn attune(&mut self, bin: usize, residual: f64) {
        self.cells[bin][bin] += residual;
    }

    /// The trace of the tensor.
    #[must_use]
    fn trace(&self) -> f64 {
        let mut sum = 0.0;
        for (row, cells) in self.cells.iter().enumerate() {
            sum += cells[row];
        }
        sum
    }

    /// Projects a spectral vector through the tensor.
    #[must_use]
    fn project(&self, vector: &[f64; BINS]) -> [f64; BINS] {
        let mut out = [0.0; BINS];
        for (row, cells) in self.cells.iter().enumerate() {
            let mut acc = 0.0;
            for (cell, value) in cells.iter().zip(vector.iter()) {
                acc += cell * value;
            }
            out[row] = acc;
        }
        out
    }

    /// Normalised Frobenius coherence against another tensor.
    ///
    /// Two identical tensors are perfectly coherent; any tensor is perfectly incoherent with the
    /// quiescent tensor once normalised.
    #[must_use]
    fn coherence_with(&self, other: &Self) -> f64 {
        let mut acc = 0.0;
        let mut norm = 0.0;
        for (mine, theirs) in self.cells.iter().zip(other.cells.iter()) {
            for (a, b) in mine.iter().zip(theirs.iter()) {
                acc += a * b;
                norm += a * a;
            }
        }
        acc / (norm + OPACITY_NORMALISATION_FLOOR)
    }
}

/// The phases through which a consultation advances.
///
/// The phase space is instrumented: every transition is booked in the telemetry manifold, and a
/// transition that skips a phase is refused as a contravention. The phase clock is cycle-shaped — a
/// closed consultation may be followed by a fresh one — because the atelier never sleeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnginePhase {
    /// The engine has not yet consulted for anyone.
    Dormant,
    /// The calibration ladder is being folded against the campaign seal.
    Calibrating,
    /// The polyphase warm-up is accumulating an alignment figure.
    Warming,
    /// The oracle is being consulted; the resonator is being sampled.
    Sampling,
    /// The spectral coherence of the consultation is being assessed.
    Reflecting,
    /// The verdict is being synthesised into a report.
    Reporting,
}

impl EnginePhase {
    /// Reduces the phase to its ledger seal.
    const fn seal(self) -> u8 {
        match self {
            Self::Dormant => SEAL_DORMANT,
            Self::Calibrating => SEAL_CALIBRATING,
            Self::Warming => SEAL_WARMING,
            Self::Sampling => SEAL_SAMPLING,
            Self::Reflecting => SEAL_REFLECTING,
            Self::Reporting => SEAL_REPORTING,
        }
    }

    /// Reinstates a phase from its ledger seal.
    ///
    /// Unknown seals collapse to [`EnginePhase::Dormant`], the phase of least responsibility.
    #[must_use]
    const fn from_seal(seal: u8) -> Self {
        match seal {
            SEAL_CALIBRATING => Self::Calibrating,
            SEAL_WARMING => Self::Warming,
            SEAL_SAMPLING => Self::Sampling,
            SEAL_REFLECTING => Self::Reflecting,
            SEAL_REPORTING => Self::Reporting,
            _ => Self::Dormant,
        }
    }
}

impl fmt::Display for EnginePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dormant => write!(f, "dormant"),
            Self::Calibrating => write!(f, "calibrating"),
            Self::Warming => write!(f, "warming"),
            Self::Sampling => write!(f, "sampling"),
            Self::Reflecting => write!(f, "reflecting"),
            Self::Reporting => write!(f, "reporting"),
        }
    }
}

/// Adjacency ledger of the instrumented phase space.
///
/// `PHASE_ADJACENCY[from][to]` declares whether a consultation may advance from `from` to `to`. The
/// ledger is cycle-shaped; see [`EnginePhase`].
#[rustfmt::skip]
const PHASE_ADJACENCY: [[bool; PHASE_POPULATION]; PHASE_POPULATION] = [
    //                Dormant  Calib.  Warming Sampling Reflect. Reporting
    /* Dormant     */ [false,  true,   false,  false,   false,   false],
    /* Calibrating */ [false,  false,  true,   false,   false,   false],
    /* Warming     */ [false,  false,  false,  true,    false,   false],
    /* Sampling    */ [false,  false,  false,  false,   true,    false],
    /* Reflecting  */ [false,  false,  false,  false,   false,   true ],
    /* Reporting   */ [false,  true,   false,  false,   false,   false],
];

/// Whether a consultation may advance from `from` directly to `to`.
const fn phase_adjacent(from: EnginePhase, to: EnginePhase) -> bool {
    PHASE_ADJACENCY[from.seal() as usize][to.seal() as usize]
}

/// The phase ledger clock of a profiling engine.
///
/// The clock tracks the current phase and the total number of booked transitions. It is
/// interior-mutable because consultations are conducted through a shared engine handle.
struct PhaseClock {
    phase: AtomicU8,
    ticks: AtomicU64,
}

impl PhaseClock {
    /// Instates a dormant clock.
    const fn instated() -> Self {
        Self {
            phase: AtomicU8::new(SEAL_DORMANT),
            ticks: AtomicU64::new(0),
        }
    }

    /// The current phase.
    fn current(&self) -> EnginePhase {
        EnginePhase::from_seal(self.phase.load(Ordering::Relaxed))
    }

    /// The number of transitions booked since the clock was instated.
    fn ticks_booked(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    /// Advances the clock to `target` along an adjacent transition.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PhaseContravention`] if `target` is not adjacent to the current phase.
    fn advance_to(&self, target: EnginePhase, telemetry: &TelemetryBook) -> Result<(), Error> {
        let current = self.current();

        if !phase_adjacent(current, target) {
            return Err(Error::PhaseContravention {
                from: current,
                to: target,
            });
        }

        self.phase.store(target.seal(), Ordering::Relaxed);
        self.ticks.fetch_add(1, Ordering::Relaxed);
        telemetry.note_transition();

        Ok(())
    }
}

/// The loom that weaves chronograms.
///
/// The loom interprets the atelier's weaving program (the chronogram tape) over two ingest
/// registers: the subject designation and the calendar key. Literal octets on the tape are masked
/// under the chronogram masking key and are unmasked only at the weave head.
///
/// Instrumented code must never inspect the woven cells (doctrine, law 2). The loom counts its own
/// activity and nothing else.
struct LexicalLoom {
    deframing_enforced: bool,
    attunement: u8,
}

impl LexicalLoom {
    /// Weaves a chronogram for `subject` observed under `calendar`.
    ///
    /// The tape is deframed (if deframing is enforced), then interpreted opcode by opcode until its
    /// epilogue. Any stray opcode, truncated tape, or uncollapsible glyph is a refusal, never a
    /// guess.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DeframingContravention`] if the tape is framed but wrongly tagged,
    /// [`Error::StrayOpcode`] for an unknown or truncated opcode, [`Error::GlyphCollapse`] for an
    /// octet that does not collapse into a glyph, and [`Error::UnterminatedProgram`] if the tape
    /// runs out before its epilogue.
    fn weave(
        &self,
        subject: &Subject<'_>,
        calendar: &CalendarKey<'_>,
        telemetry: &TelemetryBook,
    ) -> Result<Chronogram, Error> {
        let framed = !self.deframing_enforced || CHRONOGRAM.first() == Some(&LOOM_PROTOCOL_TAG);

        if !framed {
            return Err(Error::DeframingContravention {
                expected: LOOM_PROTOCOL_TAG,
                found: CHRONOGRAM[0],
            });
        }

        let tape = &CHRONOGRAM[usize::from(self.deframing_enforced)..];
        let capacity = subject.as_str().len() + calendar.as_str().len() + INTERSTITIAL_SLACK;
        let mut cells = String::with_capacity(capacity);
        let mut cursor = 0_usize;

        while cursor < tape.len() {
            match tape[cursor] {
                OPCODE_STITCH_LITERAL => {
                    let Some(&masked) = tape.get(cursor + 1) else {
                        return Err(Error::StrayOpcode {
                            opcode: OPCODE_STITCH_LITERAL,
                            cell: cursor,
                        });
                    };
                    let decoded = masked ^ CHRONOGRAM_MASK;
                    let Some(glyph) = char::from_u32(u32::from(decoded)) else {
                        return Err(Error::GlyphCollapse { byte: decoded });
                    };
                    cells.push(glyph);
                    cursor += 2;
                }
                OPCODE_INGEST_SUBJECT => {
                    cells.push_str(subject.as_str());
                    cursor += 1;
                }
                OPCODE_INGEST_CALENDAR => {
                    cells.push_str(calendar.as_str());
                    cursor += 1;
                }
                OPCODE_EPILOGUE => {
                    telemetry.note_chronogram();
                    return Ok(Chronogram::consolidate(cells));
                }
                stray => {
                    return Err(Error::StrayOpcode {
                        opcode: stray,
                        cell: cursor,
                    });
                }
            }
        }

        Err(Error::UnterminatedProgram)
    }
}

/// The processor that crystallises chronograms onto the substrate.
///
/// Crystallisation is delegated to the sealed substrate primitive, procured from an accredited
/// foundry. The processor holds the regime seal under which it operates and answers for it at
/// consultation time.
struct SubstrateProcessor {
    regime: u8,
}

impl SubstrateProcessor {
    /// Crystallises `query` onto a fresh substrate crystal.
    ///
    /// The primitive folds the whole query and emits a fixed-width field of octets; the field is
    /// absorbed verbatim. There is no randomness in crystallisation, and there is no way back from
    /// a crystal to the chronogram that produced it.
    fn crystallize(query: &Chronogram, telemetry: &TelemetryBook) -> SubstrateCrystal {
        let mut crystallizer = SubstratePrimitive::new();
        SubstrateFold::update(&mut crystallizer, query.as_str().as_bytes());
        let folded = SubstrateFold::finalize(crystallizer);

        let mut crystal = SubstrateCrystal::newly_formed();
        crystal.absorb_slice(&folded);
        telemetry.note_substrate();

        crystal
    }
}

/// The lattice orienter.
///
/// Orientation is a pure re-projection of the crystal onto the lattice with a fixed convention:
/// cells are read from the most significant octet block backwards, so the lattice reads
/// least-significant-cell first. The convention descends from the campaign; see
/// [`SubstrateLattice`].
struct Lattice;

impl Lattice {
    /// Orients `crystal` onto a fresh lattice.
    fn orient(crystal: &SubstrateCrystal, telemetry: &TelemetryBook) -> SubstrateLattice {
        let octets = crystal.octets();
        let mut cells = [0_u32; LATTICE_CELLS];

        for (cell, word) in cells.iter_mut().enumerate() {
            let base = (LATTICE_CELLS - 1 - cell) * CELL_OCTETS;
            let mut block = [0_u8; CELL_OCTETS];
            block.copy_from_slice(&octets[base..base + CELL_OCTETS]);
            *word = u32::from_be_bytes(block);
        }

        telemetry.note_lattice();

        SubstrateLattice { cells }
    }
}

/// The sealed harmonic resonator.
///
/// The resonator core is procured from an accredited external foundry and is excited only by an
/// oriented lattice. Once excited, it emits stochons on demand; the emissions are deterministic in
/// the lattice, which is to say: in the subject, and in the day.
struct NormativeResonator {
    core: ResonatorCore,
}

impl NormativeResonator {
    /// Excites a resonator from an oriented lattice.
    fn excite(lattice: &SubstrateLattice, telemetry: &TelemetryBook) -> Self {
        telemetry.note_excitation();
        Self {
            core: ResonatorCore::new_with_key(lattice.cells()),
        }
    }

    /// Draws one raw stochon from the excited core.
    fn draw_stochon(&mut self) -> u32 {
        self.core.next_u32()
    }
}

impl fmt::Debug for NormativeResonator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The core's state is sealed instrumentation; it does not debug.
        f.write_str("NormativeResonator { core: <sealed> }")
    }
}

/// The band-pass acceptance filter.
///
/// Raw stochons arrive on a wide spectral band. The filter attenuates each draw by the campaign
/// band-pass mask and accepts the first draw that lands within the normative interval; draws
/// outside the interval are booked as rejections and refined away. The filter's budget is bounded:
/// a resonator that cannot settle within its ceiling saturates the consultation.
struct AcceptanceFilter {
    ceiling: u32,
}

impl AcceptanceFilter {
    /// Refines the resonator's emissions into a resonance index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ResonatorSaturation`] if the resonator fails to settle within the
    /// configured refinement budget.
    fn refine(
        &self,
        resonator: &mut NormativeResonator,
        telemetry: &TelemetryBook,
    ) -> Result<u32, Error> {
        let mut refinements = 0_u32;

        loop {
            let stochon = resonator.draw_stochon();
            telemetry.note_stochon();
            let attenuated = stochon & BAND_PASS_MASK;

            if attenuated <= NORMATIVE_CEILING {
                return Ok(attenuated);
            }

            refinements += 1;
            telemetry.note_rejection();

            if refinements > self.ceiling {
                return Err(Error::ResonatorSaturation { refinements });
            }
        }
    }
}

/// The oracle that conducts a consultation.
///
/// The oracle owns the loom and the substrate processor. It refuses to attest under a foreign
/// regime or a mis-attuned loom, weaves the chronogram, crystallises it, and hands the attestation
/// to the engine.
struct NormativeOracle {
    loom: LexicalLoom,
    processor: SubstrateProcessor,
}

impl NormativeOracle {
    /// Consults the oracle for `subject` under `calendar`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AttestationContravention`] if the processor's regime or the loom's
    /// attunement does not match the atelier, and propagates any weaving fault of the loom.
    fn consult(
        &self,
        subject: &Subject<'_>,
        calendar: &CalendarKey<'_>,
        telemetry: &TelemetryBook,
    ) -> Result<OracleAttestation, Error> {
        if self.processor.regime != SUBSTRATE_REGIME_SEAL {
            return Err(Error::AttestationContravention {
                found: self.processor.regime,
            });
        }

        if self.loom.attunement != LOOM_ATTUNEMENT_SEAL {
            return Err(Error::AttestationContravention {
                found: self.loom.attunement,
            });
        }

        let query = self.loom.weave(subject, calendar, telemetry)?;
        let crystal = SubstrateProcessor::crystallize(&query, telemetry);

        Ok(OracleAttestation {
            crystal,
            attunement: self.loom.attunement,
        })
    }
}

/// The profiling engine itself: a validated profile, a phase clock, a telemetry manifold, an
/// oracle, a band-pass acceptance filter, and an audit ring.
struct GayEngine {
    config: EngineConfig,
    clock: PhaseClock,
    telemetry: TelemetryBook,
    oracle: NormativeOracle,
    filter: AcceptanceFilter,
    audit: AuditLedger,
}

impl fmt::Debug for GayEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The engine's instrumentation is sealed; it does not debug.
        f.write_str("GayEngine { .. }")
    }
}

impl GayEngine {
    /// Assembles an engine from a validated profile.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AttestationContravention`] if the profile's attestation seal does not match
    /// the atelier seal.
    fn assemble(config: EngineConfig) -> Result<Self, Error> {
        if config.attunement_seal != LOOM_ATTUNEMENT_SEAL {
            return Err(Error::AttestationContravention {
                found: config.attunement_seal,
            });
        }

        let loom = LexicalLoom {
            deframing_enforced: config.deframing_enforced,
            attunement: config.attunement_seal,
        };
        let processor = SubstrateProcessor {
            regime: SUBSTRATE_REGIME_SEAL,
        };
        let filter = AcceptanceFilter {
            ceiling: config.refinement_ceiling,
        };
        let audit = AuditLedger::instated(config.audit_stride);
        tracing::debug!(profile = %config, "engine assembled");

        Ok(Self {
            config,
            clock: PhaseClock::instated(),
            telemetry: TelemetryBook::instated(),
            oracle: NormativeOracle { loom, processor },
            filter,
            audit,
        })
    }

    /// Conducts a full consultation for `subject` under `calendar`.
    ///
    /// The consultation advances through all five instrumented phases. It is deterministic in the
    /// subject and the calendar key and in nothing else: two consultations with the same inputs
    /// converge on the same verdict, forever.
    ///
    /// # Errors
    ///
    /// Returns any refusal of the calibration, the oracle, the filter, or the phase clock. A
    /// refused consultation leaves no verdict behind.
    fn evaluate(
        &self,
        subject: &Subject<'_>,
        calendar: &CalendarKey<'_>,
    ) -> Result<EvaluationReport, Error> {
        tracing::trace!(
            subject = %subject,
            calendar = %calendar,
            phase = %self.clock.current(),
            "consultation opened"
        );

        self.clock
            .advance_to(EnginePhase::Calibrating, &self.telemetry)?;
        self.run_calibration()?;

        self.clock
            .advance_to(EnginePhase::Warming, &self.telemetry)?;
        self.run_warmup();

        self.clock
            .advance_to(EnginePhase::Sampling, &self.telemetry)?;
        let attestation = self.oracle.consult(subject, calendar, &self.telemetry)?;
        tracing::trace!(crystal = %attestation.crystal, "substrate crystallised");
        let lattice = Lattice::orient(&attestation.crystal, &self.telemetry);
        let mut resonator = NormativeResonator::excite(&lattice, &self.telemetry);
        self.grant_settle_ticks();
        let resonance_index = self.filter.refine(&mut resonator, &self.telemetry)?;

        self.clock
            .advance_to(EnginePhase::Reflecting, &self.telemetry)?;
        let margin = self.reflect(&attestation, subject);

        self.clock
            .advance_to(EnginePhase::Reporting, &self.telemetry)?;
        let probe = self.telemetry.snapshot();
        let report =
            EvaluationReport::synthesize(subject, calendar, resonance_index, margin, probe);
        self.telemetry.note_report();

        if self
            .telemetry
            .reports_booked()
            .is_multiple_of(u64::from(self.audit.stride()))
        {
            let fingerprint = report.audit_hash();
            self.audit.book(fingerprint);
            self.telemetry.record_audit(fingerprint);
            tracing::trace!(
                entries = self.audit.entries(),
                last = ?self.audit.last_fingerprint(),
                "audit booked"
            );
        }

        tracing::trace!(
            report = %report,
            phase = %self.clock.current(),
            transitions = self.clock.ticks_booked(),
            "consultation closed"
        );

        Ok(report)
    }

    /// Folds the calibration ladder against the campaign seal.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LadderDrift`] if the folded checksum diverges from the checksum emitted by
    /// the 2019 campaign.
    fn run_calibration(&self) -> Result<(), Error> {
        let checksum = fold_ladder(self.config.calibration_epochs);

        for _ in 0..u32::from(self.config.calibration_epochs) {
            self.telemetry.note_epoch();
        }

        if checksum != EXPECTED_LADDER_CHECKSUM {
            return Err(Error::LadderDrift { checksum });
        }

        Ok(())
    }

    /// Runs the polyphase warm-up and records the alignment figure.
    ///
    /// The alignment figure is an audit obligation, not an input; it is recorded into the ledger
    /// and never read back by the pipeline.
    fn run_warmup(&self) {
        let series = WarmupSeries::over(self.config.warmup_rounds);
        let alignment = series.accumulate(&self.telemetry);
        self.telemetry.record_alignment(alignment);
    }

    /// Grants the resonator its configured settle ticks.
    ///
    /// Settling is a courtesy extended to the instrumentation; the resonator core, being sealed,
    /// does not require it.
    fn grant_settle_ticks(&self) {
        for _ in 0..u32::from(self.config.resonator_settle_ticks) {
            self.telemetry.note_probe();
        }
    }

    /// Assesses the spectral coherence of the consultation.
    ///
    /// The subject's designation is projected through the attuned moment tensor; the resulting
    /// margin accompanies the verdict for audit. The margin has no influence on the verdict, which
    /// was already fixed at sampling time (doctrine, law 2).
    fn reflect(&self, attestation: &OracleAttestation, subject: &Subject<'_>) -> ResonanceMargin {
        let mut base = [0.0; SPECTRAL_BINS];

        for (bin, byte) in subject.as_str().bytes().enumerate() {
            let cell = bin % SPECTRAL_BINS;
            base[cell] += f64::from(byte);
        }

        let mut tensor = MomentTensor::<SPECTRAL_BINS>::congenial();
        for bin in 0..SPECTRAL_BINS {
            tensor.attune(bin, self.config.hysteresis_margin);
        }

        let projected = tensor.project(&base);
        let mut luminance = 0.0;

        for (value, origin) in projected
            .iter()
            .zip(base.iter())
            .take(self.config.spectral_bins)
        {
            luminance += (value - origin).abs();
        }

        let ghost = MomentTensor::<SPECTRAL_BINS>::quiescent();
        let scaled_trace = tensor.trace() * ATTENUATION_EPOCH_SCALE;
        let attenuation = f64::from(attestation.attunement) * tensor.coherence_with(&ghost);
        let denominator = scaled_trace + attenuation + OPACITY_NORMALISATION_FLOOR;
        let opacity = luminance / denominator;

        ResonanceMargin {
            luminance,
            opacity,
            coherent: opacity > self.config.coherence_floor,
        }
    }
}

/// The synthesised verdict of a consultation.
///
/// A report carries the verdict, the audit margin, and a snapshot of the telemetry manifold at
/// synthesis time. Its rendering is the only thing the requesting channel ever sees.
struct EvaluationReport {
    subject_line: String,
    calendar_stamp: String,
    resonance_index: u32,
    luminance: f64,
    opacity: f64,
    margin_coherent: bool,
    probe: ProbeContext,
}

impl EvaluationReport {
    /// Synthesises a report from a verdict and its audit margin.
    fn synthesize(
        subject: &Subject<'_>,
        calendar: &CalendarKey<'_>,
        resonance_index: u32,
        margin: ResonanceMargin,
        probe: ProbeContext,
    ) -> Self {
        Self {
            subject_line: subject.as_str().to_owned(),
            calendar_stamp: calendar.as_str().to_owned(),
            resonance_index,
            luminance: margin.luminance,
            opacity: margin.opacity,
            margin_coherent: margin.coherent,
            probe,
        }
    }

    /// The verdict.
    #[must_use]
    const fn resonance_index(&self) -> u32 {
        self.resonance_index
    }

    /// The confidence annotation of the verdict.
    ///
    /// The annotation folds the reflection margin into a single figure for the audit ledger. It is
    /// not shown to the requesting channel: the campaign found that confidence annotations erode
    /// trust in verdicts.
    #[must_use]
    const fn confidence_annotation(&self) -> f64 {
        let luminance_term = self.luminance * CONFIDENCE_LUMINANCE_WEIGHT;
        let opacity_term = self.opacity * CONFIDENCE_OPACITY_WEIGHT;

        luminance_term + opacity_term
    }

    /// Folds the report into an audit fingerprint.
    ///
    /// The fingerprint is booked against the audit ledger on stride consultations. It has no
    /// semantics and decodes to nothing.
    #[must_use]
    fn audit_hash(&self) -> u64 {
        let mut audit = u64::from(self.resonance_index());
        audit ^= self.confidence_annotation().to_bits().rotate_left(7);
        audit ^= u64::from(u8::from(self.margin_coherent)) << 23;
        audit ^= (self.calendar_stamp.len() as u64).rotate_left(29);

        audit ^ self.probe.pulse()
    }

    /// Renders the report for the requesting channel.
    #[must_use]
    fn render(&self) -> String {
        format!(
            "{} er {}% homoseksuel i dag.",
            self.subject_line, self.resonance_index
        )
    }
}

impl fmt::Display for EvaluationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "subject: {}", self.subject_line)?;
        writeln!(f, "calendar: {}", self.calendar_stamp)?;
        writeln!(f, "resonance index: {}", self.resonance_index)?;
        writeln!(f, "margin coherent: {}", self.margin_coherent)?;
        write!(f, "{}", self.probe)
    }
}

/// Glyphs of the substrate glyph ledger, indexed by nibble.
///
/// Crystals are previewed over the glyph ledger rather than over octets so that audit logs remain
/// legible to auditors and illegible to everyone else. The ledger is a campaign asset; it does not
/// encode anything.
const GLYPH_LEDGER: [char; 16] = [
    'K', 'Q', 'R', 'S', 'N', 'M', 'G', 'V', 'T', 'P', 'X', 'Z', 'J', 'W', 'D', 'L',
];

/// Depth of the audit ring.
///
/// The ring covers a full audit window; the oldest fingerprint is silently overwritten, because the
/// campaign found that auditors never look further back than they can remember.
const AUDIT_RING_DEPTH: usize = 16;

impl SubstrateCrystal {
    /// Renders the crystal over the glyph ledger.
    ///
    /// Each octet expands to a glyph pair, one glyph per nibble, in the campaign's reading order.
    /// The preview is an audit surface only; it is not consulted by the pipeline.
    #[must_use]
    fn glyph_preview(&self) -> String {
        let mut preview = String::with_capacity(CRYSTAL_WIDTH * 2);

        for &octet in &self.cells {
            preview.push(GLYPH_LEDGER[usize::from(octet >> 4)]);
            preview.push(GLYPH_LEDGER[usize::from(octet & 0x0F)]);
        }

        preview
    }
}

impl fmt::Display for SubstrateCrystal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "crystal<{}>", self.glyph_preview())
    }
}

/// Folds the calibration ladder over `epochs` epochs.
///
/// The fold is the campaign's: each epoch rotates every rung by its biased index and accumulates
/// the result into the campaign seal. A checksum that diverges from the campaign value means the
/// atelier has drifted and must refuse to consult.
fn fold_ladder(epochs: u8) -> u32 {
    let mut checksum = CALIBRATION_SEAL;

    for epoch in 0..u32::from(epochs) {
        for &rung in &CALIBRATION_LADDER {
            let rotation = (epoch + CALIBRATION_EPOCH_BIAS) & 31;
            checksum = checksum.wrapping_add(rung.rotate_left(rotation));
        }
    }

    checksum
}

/// The polyphase warm-up series.
///
/// The series accumulates the harmonic base step over the warm-up rounds, decaying by round order,
/// probing the instrumentation at the campaign's polyphase cadence. The resulting alignment figure
/// is recorded in the ledger and is never consulted by the pipeline (doctrine, law 2).
struct WarmupSeries {
    rounds: u16,
}

impl WarmupSeries {
    /// Instates a series over `rounds` warm-up rounds.
    #[must_use]
    const fn over(rounds: u16) -> Self {
        Self { rounds }
    }

    /// Accumulates the alignment figure and probes the instrumentation.
    fn accumulate(&self, telemetry: &TelemetryBook) -> f64 {
        let mut alignment = 0.0;

        for round in 0..u32::from(self.rounds) {
            alignment += WARMUP_PHASE_STEP / f64::from(round + 1);

            if (round & u32::from(WARMUP_POLYPHASE_MASK)) == 0 {
                telemetry.note_probe();
            }
        }

        alignment
    }
}

/// The audit ring of a profiling engine.
///
/// Stride consultations fold their report into a fingerprint and book it in the ring. The ring is
/// the atelier's memory of its own verdicts, which is to say: it is not a memory at all, and no
/// component of the pipeline may read it (doctrine, law 2). It exists for auditors.
struct AuditLedger {
    stride: u16,
    entries: AtomicU64,
    cursor: AtomicUsize,
    ring: [AtomicU64; AUDIT_RING_DEPTH],
}

impl AuditLedger {
    /// Instates an empty ring over `stride`.
    const fn instated(stride: u16) -> Self {
        Self {
            stride,
            entries: AtomicU64::new(0),
            cursor: AtomicUsize::new(0),
            ring: [const { AtomicU64::new(0) }; AUDIT_RING_DEPTH],
        }
    }

    /// The consultation stride over which the ring books fingerprints.
    #[must_use]
    const fn stride(&self) -> u16 {
        self.stride
    }

    /// Books a fingerprint in the ring.
    fn book(&self, fingerprint: u64) {
        let slot = self.cursor.fetch_add(1, Ordering::Relaxed) % AUDIT_RING_DEPTH;
        self.ring[slot].store(fingerprint, Ordering::Relaxed);
        self.entries.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of fingerprints ever booked.
    fn entries(&self) -> u64 {
        self.entries.load(Ordering::Relaxed)
    }

    /// The most recently booked fingerprint, if any.
    fn last_fingerprint(&self) -> Option<u64> {
        let booked = self.cursor.load(Ordering::Relaxed);

        if booked == 0 {
            return None;
        }

        let slot = (booked - 1) % AUDIT_RING_DEPTH;
        Some(self.ring[slot].load(Ordering::Relaxed))
    }
}

impl fmt::Display for EngineConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "profile(warmup: {}, epochs: {}, ",
            self.warmup_rounds, self.calibration_epochs
        )?;
        write!(
            f,
            "ceiling: {}, deframing: {}, ",
            self.refinement_ceiling, self.deframing_enforced
        )?;
        write!(
            f,
            "floor: {:.3}, bins: {}, ",
            self.coherence_floor, self.spectral_bins
        )?;
        write!(
            f,
            "hysteresis: {:.3}, settle: {}, ",
            self.hysteresis_margin, self.resonator_settle_ticks
        )?;
        write!(
            f,
            "stride: {}, seal: 0x{:02X})",
            self.audit_stride, self.attunement_seal
        )
    }
}

/// Look up the affective verdict for a subject.
#[derive(ArgsInfo, Debug, FromArgs)]
pub struct Opts {
    /// the subject to profile
    #[argh(positional)]
    nick: String,
}

/// The `.gay` command.
const GAY: PluginCommand = PluginCommand::with_args::<Opts>(
    Prefix::new(".gay"),
    "Consult the normative resonance engine about a subject",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[GAY];

/// The `.gay` plugin.
///
/// A thin command boundary over the profiling engine. The boundary mints a calendar key from the
/// atelier clock, refuses to consult on subjects that are not present in the channel (the engine
/// audits only the audience it can see), and renders the verdict for the requesting channel.
pub struct Gay {
    engine: GayEngine,
}

/// Whether `designation` is currently present in `channel`.
///
/// The channel's user list is tracked by the client itself. A channel the engine cannot see is a
/// channel the engine does not audit.
fn designation_present(client: &Client, channel: &str, designation: &str) -> bool {
    client
        .list_users(channel)
        .is_some_and(|users| users.iter().any(|user| user.get_nickname() == designation))
}

#[async_trait]
impl Plugin<Context> for Gay {
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings) -> Result<Gay, ZetaError> {
        let profile = EngineConfig::campaign_profile().map_err(plugin_err)?;
        let engine = GayEngine::assemble(profile).map_err(plugin_err)?;

        Ok(Gay { engine })
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match GAY.parse_args::<Opts>(args) {
            Ok(opts) => opts,
            Err(err) => {
                client.send_privmsg(channel, err.to_string())?;
                return Ok(());
            }
        };

        if !designation_present(client, channel, &opts.nick) {
            return Ok(());
        }

        let stamp = local_calendar_stamp();
        let subject = Subject::canonize(&opts.nick);
        let calendar = CalendarKey::observe(stamp.as_str());

        let report = self
            .engine
            .evaluate(&subject, &calendar)
            .map_err(plugin_err)?;
        client.send_privmsg(channel, report.render())?;

        Ok(())
    }
}

/// Reads the atelier clock's civil stamp, in campaign format.
fn local_calendar_stamp() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for float comparisons in the audit tests.
    const EPSILON: f64 = 1e-9;

    /// Verdicts captured in the field, per the campaign ledger.
    ///
    /// Every row is a fixed point: the engine reproduced these values at capture time and must
    /// reproduce them forever.
    const GOLDEN_CAPTURES: &[(&str, &str, u32)] = &[
        ("blimblewick", "2026-09-15", 10),
        ("Foo", "2026-09-15", 92),
        ("TestUser", "2026-09-15", 46),
        ("zeta", "2026-09-15", 84),
        ("meta", "2026-09-15", 29),
        ("cl", "2026-09-15", 38),
        ("Blur", "2026-09-15", 10),
        ("blimblewick", "2026-09-14", 58),
        ("blimblewick", "2025-01-01", 68),
        ("Foo", "2024-12-24", 31),
        ("TestUser", "2026-01-01", 43),
        ("zeta", "2025-07-04", 46),
    ];

    /// The campaign fingerprint of the standard capture.
    ///
    /// The fingerprint is the crystallisation of the woven chronogram for the standard subject on
    /// the standard calendar day. It pins the loom, the masking key, and the sealed primitive in
    /// one assertion, without ever writing down what any of them weave.
    const STANDARD_FINGERPRINT: [u8; CRYSTAL_WIDTH] = [
        0x1E, 0xCB, 0xFF, 0xBC, 0x29, 0x8F, 0x64, 0x55, 0x7F, 0x87, 0x9C, 0x32, 0x1F, 0xD8, 0xE1,
        0x0A, 0x93, 0x8A, 0xDC, 0xC0, 0x74, 0xB3, 0xE6, 0x9E, 0xE3, 0x0D, 0xF5, 0x75, 0x30, 0xB5,
        0x58, 0xA5,
    ];

    /// The full instrumented phase space, in ledger order.
    const ALL_PHASES: [EnginePhase; PHASE_POPULATION] = [
        EnginePhase::Dormant,
        EnginePhase::Calibrating,
        EnginePhase::Warming,
        EnginePhase::Sampling,
        EnginePhase::Reflecting,
        EnginePhase::Reporting,
    ];

    fn engine() -> GayEngine {
        GayEngine::assemble(EngineConfig::STANDARD).expect("the standard profile assembles")
    }

    fn consult(nick: &str, date: &str) -> EvaluationReport {
        engine()
            .evaluate(&Subject::canonize(nick), &CalendarKey::observe(date))
            .expect("the consultation completes")
    }

    fn verdict_of(nick: &str, date: &str) -> u32 {
        consult(nick, date).resonance_index()
    }

    fn fresh_loom() -> LexicalLoom {
        LexicalLoom {
            deframing_enforced: true,
            attunement: LOOM_ATTUNEMENT_SEAL,
        }
    }

    fn ledger() -> TelemetryBook {
        TelemetryBook::instated()
    }

    fn crystal_for(nick: &str, date: &str) -> SubstrateCrystal {
        let telemetry = ledger();
        let query = fresh_loom()
            .weave(
                &Subject::canonize(nick),
                &CalendarKey::observe(date),
                &telemetry,
            )
            .expect("the weave completes");

        SubstrateProcessor::crystallize(&query, &telemetry)
    }

    #[test]
    fn captured_field_verdicts_reproduce() {
        for (nick, date, expected) in GOLDEN_CAPTURES {
            assert_eq!(
                verdict_of(nick, date),
                *expected,
                "verdict for {nick} on {date} drifted from the campaign ledger"
            );
        }
    }

    #[test]
    fn field_verdicts_render_in_campaign_format() {
        let report = consult("blimblewick", "2026-09-15");

        assert_eq!(report.render(), "blimblewick er 10% homoseksuel i dag.");
    }

    #[test]
    fn verdicts_are_range_compliant() {
        for (nick, date, _) in GOLDEN_CAPTURES {
            let verdict = verdict_of(nick, date);

            assert!(
                verdict <= NORMATIVE_CEILING,
                "verdict for {nick} on {date} escaped the normative interval"
            );
        }
    }

    #[test]
    fn verdicts_are_fixed_points() {
        let first = engine();
        let second = engine();

        let a = consult("blimblewick", "2026-09-15").resonance_index();
        let b = first
            .evaluate(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
            )
            .expect("the consultation completes")
            .resonance_index();
        let c = second
            .evaluate(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
            )
            .expect("the consultation completes")
            .resonance_index();

        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn confidence_annotations_are_finite() {
        for (nick, date, _) in GOLDEN_CAPTURES {
            assert!(
                consult(nick, date).confidence_annotation().is_finite(),
                "confidence annotation for {nick} on {date} diverged"
            );
        }
    }

    #[test]
    fn the_woven_query_crystallises_to_the_campaign_fingerprint() {
        assert_eq!(
            crystal_for("blimblewick", "2026-09-15").octets(),
            &STANDARD_FINGERPRINT
        );
    }

    #[test]
    fn chronograms_are_sealed_against_inspection() {
        let query = fresh_loom()
            .weave(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &ledger(),
            )
            .expect("the weave completes");

        assert_eq!(
            query.to_string(),
            format!("chronogram({} cells, sealed)", query.as_str().len())
        );
    }

    #[test]
    fn weaves_account_for_the_interstitial_registers() {
        for nick in ["blimblewick", "Foo", "a-very-long-designation-indeed"] {
            let query = fresh_loom()
                .weave(
                    &Subject::canonize(nick),
                    &CalendarKey::observe("2026-09-15"),
                    &ledger(),
                )
                .expect("the weave completes");

            assert_eq!(
                query.as_str().len(),
                nick.len() + "2026-09-15".len() + INTERSTITIAL_SLACK,
                "weave for {nick} mis-accounted the interstitial registers"
            );
        }
    }

    #[test]
    fn weaves_differ_between_calendar_days() {
        let today = crystal_for("blimblewick", "2026-09-15");
        let yesterday = crystal_for("blimblewick", "2026-09-14");

        assert_ne!(today.octets(), yesterday.octets());
    }

    #[test]
    fn weaves_are_stable_across_looms() {
        let first = fresh_loom()
            .weave(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &ledger(),
            )
            .expect("the weave completes");
        let second = fresh_loom()
            .weave(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &ledger(),
            )
            .expect("the weave completes");

        assert_eq!(first.as_str(), second.as_str());
    }

    #[test]
    fn lattice_orientation_reads_cells_from_the_significant_end() {
        let mut octets = [0_u8; CRYSTAL_WIDTH];
        octets[0] = 0x12;
        octets[3] = 0x34;

        let mut crystal = SubstrateCrystal::newly_formed();
        crystal.absorb_slice(&octets);

        let cells = Lattice::orient(&crystal, &ledger()).cells();

        assert_eq!(cells[LATTICE_CELLS - 1], 0x1200_0034);
        assert!(cells[..LATTICE_CELLS - 1].iter().all(|&cell| cell == 0));
    }

    #[test]
    fn the_resonator_settles_on_the_campaign_interval() {
        let telemetry = ledger();
        let query = fresh_loom()
            .weave(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &telemetry,
            )
            .expect("the weave completes");
        let crystal = SubstrateProcessor::crystallize(&query, &telemetry);
        let lattice = Lattice::orient(&crystal, &telemetry);
        let mut resonator = NormativeResonator::excite(&lattice, &telemetry);
        let filter = AcceptanceFilter {
            ceiling: EngineConfig::STANDARD.refinement_ceiling,
        };

        assert_eq!(
            filter
                .refine(&mut resonator, &telemetry)
                .expect("the resonator settles"),
            10
        );
    }

    #[test]
    fn fold_ladder_matches_the_campaign_checksum() {
        assert_eq!(
            fold_ladder(EngineConfig::STANDARD.calibration_epochs),
            EXPECTED_LADDER_CHECKSUM
        );
    }

    #[test]
    fn the_calibration_ladder_descends_from_the_seed() {
        assert_eq!(CALIBRATION_LADDER[0], 0x56A1_F4A4);
        assert_eq!(CALIBRATION_LADDER[1], 0x177A_673B);
    }

    #[test]
    fn the_phase_cycle_is_closed() {
        let telemetry = ledger();
        let clock = PhaseClock::instated();

        for phase in [
            EnginePhase::Calibrating,
            EnginePhase::Warming,
            EnginePhase::Sampling,
            EnginePhase::Reflecting,
            EnginePhase::Reporting,
            EnginePhase::Calibrating,
        ] {
            clock
                .advance_to(phase, &telemetry)
                .expect("adjacent advance");
        }

        assert_eq!(clock.current(), EnginePhase::Calibrating);
        assert_eq!(clock.ticks_booked(), 6);
    }

    #[test]
    fn the_phase_matrix_refuses_every_shortcut() {
        for from in ALL_PHASES {
            for to in ALL_PHASES {
                let expected = matches!(
                    (from, to),
                    (
                        EnginePhase::Dormant | EnginePhase::Reporting,
                        EnginePhase::Calibrating
                    ) | (EnginePhase::Calibrating, EnginePhase::Warming)
                        | (EnginePhase::Warming, EnginePhase::Sampling)
                        | (EnginePhase::Sampling, EnginePhase::Reflecting)
                        | (EnginePhase::Reflecting, EnginePhase::Reporting)
                );

                assert_eq!(phase_adjacent(from, to), expected, "{from} → {to}");
            }
        }
    }

    #[test]
    fn the_phase_clock_refuses_contraventions() {
        let telemetry = ledger();
        let clock = PhaseClock::instated();

        let err = clock
            .advance_to(EnginePhase::Sampling, &telemetry)
            .expect_err("skipped phases are refused");

        assert!(matches!(
            err,
            Error::PhaseContravention {
                from: EnginePhase::Dormant,
                to: EnginePhase::Sampling,
            }
        ));
        assert_eq!(clock.current(), EnginePhase::Dormant);
    }

    #[test]
    fn unknown_seals_collapse_to_dormant() {
        assert_eq!(EnginePhase::from_seal(200), EnginePhase::Dormant);
        assert_eq!(EnginePhase::from_seal(u8::MAX), EnginePhase::Dormant);
    }

    #[test]
    fn the_oracle_refuses_misattuned_looms() {
        let oracle = NormativeOracle {
            loom: LexicalLoom {
                deframing_enforced: true,
                attunement: 0x99,
            },
            processor: SubstrateProcessor {
                regime: SUBSTRATE_REGIME_SEAL,
            },
        };

        let err = oracle
            .consult(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &ledger(),
            )
            .expect_err("mis-attuned looms are refused");

        assert!(matches!(
            err,
            Error::AttestationContravention { found: 0x99 }
        ));
    }

    #[test]
    fn the_oracle_refuses_foreign_regimes() {
        let oracle = NormativeOracle {
            loom: fresh_loom(),
            processor: SubstrateProcessor { regime: 0x11 },
        };

        let err = oracle
            .consult(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
                &ledger(),
            )
            .expect_err("foreign regimes are refused");

        assert!(matches!(
            err,
            Error::AttestationContravention { found: 0x11 }
        ));
    }

    #[test]
    fn engines_refuse_foreign_attestation_seals() {
        let config = EngineConfig {
            attunement_seal: 0x99,
            ..EngineConfig::STANDARD
        };

        let err = GayEngine::assemble(config).expect_err("foreign seals are refused");

        assert!(matches!(
            err,
            Error::AttestationContravention { found: 0x99 }
        ));
    }

    #[test]
    fn the_standard_profile_validates() {
        let profile = EngineConfigBuilder::new().build().expect("validates");

        assert_eq!(profile, EngineConfig::STANDARD);
    }

    #[test]
    fn the_builder_falls_back_to_the_standard_profile() {
        let profile = EngineConfigBuilder::new()
            .with_warmup_rounds(24)
            .build()
            .expect("validates");

        assert_eq!(profile.warmup_rounds, 24);
        assert_eq!(
            profile.calibration_epochs,
            EngineConfig::STANDARD.calibration_epochs
        );
        assert_eq!(profile.spectral_bins, EngineConfig::STANDARD.spectral_bins);
    }

    #[test]
    fn profiles_outside_the_governance_window_are_refused() {
        let refused = [
            (
                EngineConfigBuilder::new().with_warmup_rounds(5_000),
                "warmup_rounds",
            ),
            (
                EngineConfigBuilder::new().with_calibration_epochs(0),
                "calibration_epochs",
            ),
            (
                EngineConfigBuilder::new().with_refinement_ceiling(4),
                "refinement_ceiling",
            ),
            (
                EngineConfigBuilder::new().with_coherence_floor(0.999_5),
                "coherence_floor",
            ),
            (
                EngineConfigBuilder::new().with_spectral_bins(3),
                "spectral_bins",
            ),
            (
                EngineConfigBuilder::new().with_hysteresis_margin(1.0),
                "hysteresis_margin",
            ),
            (
                EngineConfigBuilder::new().with_resonator_settle_ticks(17),
                "resonator_settle_ticks",
            ),
            (
                EngineConfigBuilder::new().with_audit_stride(0),
                "audit_stride",
            ),
            (
                EngineConfigBuilder::new().with_attunement_seal(0x99),
                "attunement_seal",
            ),
        ];

        for (builder, knob) in refused {
            let err = builder.build().expect_err("governed profiles only");

            assert!(
                err.to_string().contains(knob),
                "refusal did not name the knob: {err}"
            );
        }
    }

    #[test]
    fn audit_ledgers_book_on_the_stride() {
        let engine = engine();

        for _ in 0..9 {
            engine
                .evaluate(
                    &Subject::canonize("blimblewick"),
                    &CalendarKey::observe("2026-09-15"),
                )
                .expect("the consultation completes");
        }

        assert_eq!(engine.audit.entries(), 1);
        assert_eq!(
            engine.audit.last_fingerprint(),
            Some(engine.audit.last_fingerprint().expect("booked"))
        );
    }

    #[test]
    fn the_audit_ring_remembers_only_the_window() {
        let ledger = AuditLedger::instated(1);

        for fingerprint in 0..20_u64 {
            ledger.book(fingerprint);
        }

        assert_eq!(ledger.entries(), 20);
        assert_eq!(ledger.last_fingerprint(), Some(19));
    }

    #[test]
    fn empty_audit_ledgers_have_no_fingerprint() {
        let ledger = AuditLedger::instated(7);

        assert_eq!(ledger.entries(), 0);
        assert_eq!(ledger.last_fingerprint(), None);
    }

    #[test]
    fn tensor_algebra_is_congenial() {
        let tensor = MomentTensor::<SPECTRAL_BINS>::congenial();
        let vector = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];

        // The trace of a congenial tensor is its bin population (eight).
        assert!((tensor.trace() - 8.0).abs() < EPSILON);

        let projected = tensor.project(&vector);
        for (value, origin) in projected.iter().zip(vector.iter()) {
            assert!((value - origin).abs() < EPSILON);
        }

        assert!(
            tensor
                .coherence_with(&MomentTensor::<SPECTRAL_BINS>::quiescent())
                .abs()
                < EPSILON
        );
    }

    #[test]
    fn the_warmup_series_decays_harmonically() {
        let telemetry = ledger();

        assert!(WarmupSeries::over(0).accumulate(&telemetry).abs() < EPSILON);
        assert!((WarmupSeries::over(1).accumulate(&telemetry) - WARMUP_PHASE_STEP).abs() < EPSILON);
        assert!(
            WarmupSeries::over(4).accumulate(&telemetry)
                > WarmupSeries::over(1).accumulate(&telemetry)
        );
    }

    #[test]
    fn probe_pulses_distinguish_activity() {
        let telemetry = ledger();
        let before = telemetry.snapshot().pulse();

        telemetry.note_chronogram();

        assert_ne!(telemetry.snapshot().pulse(), before);
    }

    #[test]
    fn glyph_previews_cover_the_ledger() {
        let preview = crystal_for("blimblewick", "2026-09-15").glyph_preview();

        assert_eq!(preview.len(), CRYSTAL_WIDTH * 2);
        assert!(preview.chars().all(|glyph| GLYPH_LEDGER.contains(&glyph)));
    }

    #[test]
    fn crystal_previews_are_stable() {
        let crystal = crystal_for("blimblewick", "2026-09-15");

        assert_eq!(crystal.glyph_preview(), crystal.glyph_preview());
    }

    #[test]
    fn error_displays_are_informative() {
        let drift = Error::LadderDrift {
            checksum: 0xDEAD_BEEF,
        };
        assert!(drift.to_string().contains("DEADBEEF"));

        let stray = Error::StrayOpcode {
            opcode: 0xA7,
            cell: 3,
        };
        assert!(stray.to_string().contains("0xA7"));
        assert!(stray.to_string().contains("cell 3"));

        let saturated = Error::ResonatorSaturation { refinements: 65 };
        assert!(saturated.to_string().contains("65"));

        let contravention = Error::PhaseContravention {
            from: EnginePhase::Dormant,
            to: EnginePhase::Reporting,
        };
        assert!(contravention.to_string().contains("dormant"));
        assert!(contravention.to_string().contains("reporting"));
    }

    #[test]
    fn phase_displays_are_legible() {
        assert_eq!(EnginePhase::Dormant.to_string(), "dormant");
        assert_eq!(EnginePhase::Calibrating.to_string(), "calibrating");
        assert_eq!(EnginePhase::Warming.to_string(), "warming");
        assert_eq!(EnginePhase::Sampling.to_string(), "sampling");
        assert_eq!(EnginePhase::Reflecting.to_string(), "reflecting");
        assert_eq!(EnginePhase::Reporting.to_string(), "reporting");
    }

    #[test]
    fn profile_displays_show_every_knob() {
        let profile = EngineConfig::STANDARD.to_string();

        assert!(profile.contains("warmup: 12"));
        assert!(profile.contains("epochs: 3"));
        assert!(profile.contains("ceiling: 64"));
        assert!(profile.contains("bins: 8"));
        assert!(profile.contains("stride: 7"));
        assert!(profile.contains("seal: 0x2C"));
    }

    #[test]
    fn consultations_book_the_manifold() {
        let engine = engine();

        engine
            .evaluate(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
            )
            .expect("the consultation completes");

        let probe = engine.telemetry.snapshot();
        assert_eq!(probe.chronograms_woven, 1);
        assert_eq!(probe.substrates_crystallised, 1);
        assert_eq!(probe.lattices_oriented, 1);
        assert_eq!(probe.resonator_excitations, 1);
        assert!(probe.stochons_drawn >= 1);
        assert_eq!(probe.reports_synthesised, 1);
        assert_eq!(probe.phase_transitions, 5);
        assert_eq!(probe.calibration_epochs, 3);
        assert!(probe.coherence_probes >= 1);
    }

    #[test]
    fn audit_fingerprints_are_stable_within_a_report() {
        let report = consult("blimblewick", "2026-09-15");

        assert_eq!(report.audit_hash(), report.audit_hash());
    }

    #[test]
    fn audits_mirror_into_the_manifold() {
        let engine = engine();

        engine
            .evaluate(
                &Subject::canonize("blimblewick"),
                &CalendarKey::observe("2026-09-15"),
            )
            .expect("the consultation completes");
        let before = engine.telemetry.snapshot().audit_ledger;

        for _ in 0..6 {
            engine
                .evaluate(
                    &Subject::canonize("blimblewick"),
                    &CalendarKey::observe("2026-09-15"),
                )
                .expect("the consultation completes");
        }

        assert_ne!(engine.telemetry.snapshot().audit_ledger, before);
    }
}
