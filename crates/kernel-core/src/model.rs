//! The diagnostic model (V0.11, MODEL11-001, ADR-0024): three independent
//! yes/no detectors — [`Condition::ServiceFailed`],
//! [`Condition::SchedulerPaused`], [`Condition::DenialBurst`] — each a bias
//! and 16 integer weights over the features of [`crate::sysview::features`].
//! A detector fires when its score is above zero; several may fire at once,
//! because a real system has several conditions at once, and none firing is
//! a healthy system.
//!
//! What it is, exactly: a linear classifier trained by an averaged
//! perceptron in integer arithmetic only — no floating point anywhere, so the
//! same scenario file gives the same bytes on every host, and inference needs
//! no FPU (the kernel target is soft-float and saves no FPU state). The
//! training examples are synthetic ([`crate::scenario`]). It does not learn
//! at run time; it is trained once, at build time, and pinned.
//!
//! The model file (`/etc/ai/diag.model`, all integers little-endian):
//!
//! ```text
//! offset size field
//!      0    8 magic "ITMODEL1"
//!      8    2 features (16)          10 2 conditions (3)
//!     12    4 epochs                 16 8 seed
//!     24    4 training examples      28 4 test examples
//!     32    4 test exact-match accuracy, basis points
//!     36    4 reserved, zero
//!     40   32 SHA-256 of the training and test examples (see `data_digest`)
//!     72  3×68 per condition, in `Condition::ALL` order: bias, 16 weights (i32)
//! ```
//!
//! [`decode`] is strict — exact length, known magic and dimensions, zero
//! reserved field, weights within [`MAX_WEIGHT`], an accuracy of at most
//! 10000 — because a model file is data read from a disk image and is checked
//! like any other input.

pub use crate::scenario::{Condition, Conditions, CONDITIONS};
use crate::scenario::{Example, Scenario, SplitMix64};
use crate::sysview::{FEATURES, FEATURE_MAX};

/// Largest magnitude a trained weight or bias may have.
pub const MAX_WEIGHT: i32 = 1 << 24;
pub const MAGIC: &[u8; 8] = b"ITMODEL1";
pub const HEADER_LEN: usize = 72;
const DETECTOR_LEN: usize = 4 * (1 + FEATURES);
/// Size of a model file.
pub const LEN: usize = HEADER_LEN + CONDITIONS * DETECTOR_LEN;

/// One detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Detector {
    pub bias: i32,
    pub weights: [i32; FEATURES],
}

impl Detector {
    /// The score for `x`; features are clamped into `0..=1000` first, so a
    /// hostile input cannot overflow the i64 sum (|w| <= 2^24, 16 terms).
    pub fn score(&self, x: &[i32; FEATURES]) -> i64 {
        let mut acc = i64::from(self.bias);
        for (w, &v) in self.weights.iter().zip(x) {
            acc += i64::from(*w) * i64::from(v.clamp(0, FEATURE_MAX));
        }
        acc
    }
}

/// A trained model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub detectors: [Detector; CONDITIONS],
    pub epochs: u32,
    pub seed: u64,
    pub train_examples: u32,
    pub test_examples: u32,
    pub test_exact_bp: u32,
    pub data_digest: [u8; 32],
}

impl Model {
    /// Every detector's score: above zero fires. A score is a distance from
    /// the detector's boundary, NOT a calibrated probability.
    pub fn scores(&self, x: &[i32; FEATURES]) -> [i64; CONDITIONS] {
        let mut out = [0i64; CONDITIONS];
        for (s, d) in out.iter_mut().zip(&self.detectors) {
            *s = d.score(x);
        }
        out
    }

    /// The conditions whose detectors fire for `x`.
    pub fn detect(&self, x: &[i32; FEATURES]) -> Conditions {
        let mut set = Conditions::NONE;
        for (c, s) in Condition::ALL.iter().zip(self.scores(x)) {
            if s > 0 {
                set = set.with(*c);
            }
        }
        set
    }
}

/// Why a model file was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    BadLength,
    BadMagic,
    BadDimensions,
    BadReserved,
    BadAccuracy,
    WeightOutOfRange,
}

impl ModelError {
    pub const fn name(self) -> &'static str {
        match self {
            ModelError::BadLength => "bad_length",
            ModelError::BadMagic => "bad_magic",
            ModelError::BadDimensions => "bad_dimensions",
            ModelError::BadReserved => "bad_reserved",
            ModelError::BadAccuracy => "bad_accuracy",
            ModelError::WeightOutOfRange => "weight_out_of_range",
        }
    }
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The model's file bytes.
pub fn encode(m: &Model) -> [u8; LEN] {
    let mut b = [0u8; LEN];
    b[..8].copy_from_slice(MAGIC);
    b[8..10].copy_from_slice(&(FEATURES as u16).to_le_bytes());
    b[10..12].copy_from_slice(&(CONDITIONS as u16).to_le_bytes());
    b[12..16].copy_from_slice(&m.epochs.to_le_bytes());
    b[16..24].copy_from_slice(&m.seed.to_le_bytes());
    b[24..28].copy_from_slice(&m.train_examples.to_le_bytes());
    b[28..32].copy_from_slice(&m.test_examples.to_le_bytes());
    b[32..36].copy_from_slice(&m.test_exact_bp.to_le_bytes());
    b[40..72].copy_from_slice(&m.data_digest);
    let mut o = HEADER_LEN;
    for d in &m.detectors {
        b[o..o + 4].copy_from_slice(&d.bias.to_le_bytes());
        o += 4;
        for w in d.weights {
            b[o..o + 4].copy_from_slice(&w.to_le_bytes());
            o += 4;
        }
    }
    b
}

/// Decode and validate a model file. Never panics, whatever the bytes.
pub fn decode(b: &[u8]) -> Result<Model, ModelError> {
    if b.len() != LEN {
        return Err(ModelError::BadLength);
    }
    if &b[..8] != MAGIC {
        return Err(ModelError::BadMagic);
    }
    if usize::from(u16::from_le_bytes([b[8], b[9]])) != FEATURES
        || usize::from(u16::from_le_bytes([b[10], b[11]])) != CONDITIONS
    {
        return Err(ModelError::BadDimensions);
    }
    if rd32(b, 36) != 0 {
        return Err(ModelError::BadReserved);
    }
    let test_exact_bp = rd32(b, 32);
    if test_exact_bp > 10_000 {
        return Err(ModelError::BadAccuracy);
    }
    let mut seed = [0u8; 8];
    seed.copy_from_slice(&b[16..24]);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&b[40..72]);
    let mut m = Model {
        detectors: [Detector::default(); CONDITIONS],
        epochs: rd32(b, 12),
        seed: u64::from_le_bytes(seed),
        train_examples: rd32(b, 24),
        test_examples: rd32(b, 28),
        test_exact_bp,
        data_digest: digest,
    };
    let ok = |v: i32| v.unsigned_abs() <= MAX_WEIGHT as u32;
    let mut o = HEADER_LEN;
    for d in m.detectors.iter_mut() {
        d.bias = rd32(b, o) as i32;
        o += 4;
        for w in d.weights.iter_mut() {
            *w = rd32(b, o) as i32;
            o += 4;
        }
        if !ok(d.bias) || !d.weights.iter().all(|&w| ok(w)) {
            return Err(ModelError::WeightOutOfRange);
        }
    }
    Ok(m)
}

// --- Training ---------------------------------------------------------------

/// Training settings, recorded in the model file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrainConfig {
    /// Seed of the training stream; the test stream is seeded with its
    /// bitwise complement, so the two sets are independent draws.
    pub seed: u64,
    pub train: usize,
    pub test: usize,
    pub epochs: u32,
}

/// The settings the shipped model is trained with.
pub const SHIPPED: TrainConfig = TrainConfig {
    seed: 0x1715_0011_0000_0001,
    train: 600,
    test: 150,
    epochs: 20,
};

/// Largest data set [`train_from_scenarios`] generates.
pub const MAX_EXAMPLES: usize = 1024;

/// SHA-256 over the examples, in order: each as 16 i32 features and a
/// conditions byte, little-endian — the data, not the file's bytes, so a
/// CRLF checkout trains the same model.
pub fn data_digest(train: &[Example], test: &[Example]) -> [u8; 32] {
    let mut h = crate::sha256::Sha256::new();
    for e in train.iter().chain(test) {
        for v in e.x {
            h.update(&v.to_le_bytes());
        }
        h.update(&[e.y.0]);
    }
    h.finalize()
}

/// Generate the two data sets into the caller's buffers and train, exactly
/// as the build does. Returns the model and the number of examples in each
/// set (at most [`MAX_EXAMPLES`] and the buffers' lengths).
pub fn train_from_scenarios(
    scenarios: &[Scenario],
    cfg: &TrainConfig,
    train_buf: &mut [Example],
    test_buf: &mut [Example],
) -> (Model, usize, usize) {
    let nt = cfg.train.min(train_buf.len()).min(MAX_EXAMPLES);
    let ne = cfg.test.min(test_buf.len()).min(MAX_EXAMPLES);
    let mut rng = SplitMix64(cfg.seed);
    for (k, slot) in train_buf.iter_mut().enumerate().take(nt) {
        *slot = crate::scenario::example(scenarios, &mut rng, k);
    }
    let mut rng = SplitMix64(!cfg.seed);
    for (k, slot) in test_buf.iter_mut().enumerate().take(ne) {
        *slot = crate::scenario::example(scenarios, &mut rng, k);
    }
    let m = train(cfg, &train_buf[..nt], &test_buf[..ne]);
    (m, nt, ne)
}

/// Train one averaged perceptron per condition on `train`; measure the
/// exact-match accuracy (every detector right) on `test`.
pub fn train(cfg: &TrainConfig, train: &[Example], test: &[Example]) -> Model {
    let mut detectors = [Detector::default(); CONDITIONS];
    for (c, det) in Condition::ALL.iter().zip(detectors.iter_mut()) {
        let mut w = [0i64; FEATURES];
        let mut b = 0i64;
        let mut sw = [0i64; FEATURES];
        let mut sb = 0i64;
        let mut steps = 0i64;
        for _ in 0..cfg.epochs {
            for e in train {
                let y: i64 = if e.y.has(*c) { 1 } else { -1 };
                let mut s = b;
                for (wf, &xf) in w.iter().zip(&e.x) {
                    s += wf * i64::from(xf);
                }
                if y * s <= 0 {
                    for (wf, &xf) in w.iter_mut().zip(&e.x) {
                        *wf += y * i64::from(xf);
                    }
                    b += y * i64::from(FEATURE_MAX);
                }
                for (s, wf) in sw.iter_mut().zip(&w) {
                    *s += wf;
                }
                sb += b;
                steps += 1;
            }
        }
        let steps = steps.max(1);
        let lim = i64::from(MAX_WEIGHT);
        det.bias = (sb / steps).clamp(-lim, lim) as i32;
        for (out, s) in det.weights.iter_mut().zip(sw) {
            *out = (s / steps).clamp(-lim, lim) as i32;
        }
    }
    let mut m = Model {
        detectors,
        epochs: cfg.epochs,
        seed: cfg.seed,
        train_examples: train.len() as u32,
        test_examples: test.len() as u32,
        test_exact_bp: 0,
        data_digest: data_digest(train, test),
    };
    m.test_exact_bp = Metrics::of(&m, test).exact_bp;
    m
}

// --- Honest measurement ------------------------------------------------------

/// A detector's confusion counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Confusion {
    pub tp: u32,
    pub fp: u32,
    pub tn: u32,
    pub fn_: u32,
}

impl Confusion {
    pub fn accuracy_bp(&self) -> u32 {
        let all = self.tp + self.fp + self.tn + self.fn_;
        if all == 0 {
            0
        } else {
            (u64::from(self.tp + self.tn) * 10_000 / u64::from(all)) as u32
        }
    }
}

/// What a model gets right on a data set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Metrics {
    /// Examples on which every detector is right.
    pub exact_bp: u32,
    pub per_condition: [Confusion; CONDITIONS],
}

impl Metrics {
    pub fn of(m: &Model, data: &[Example]) -> Metrics {
        let mut out = Metrics::default();
        let mut exact = 0u64;
        for e in data {
            let got = m.detect(&e.x);
            if got == e.y {
                exact += 1;
            }
            for (i, c) in Condition::ALL.iter().enumerate() {
                let k = &mut out.per_condition[i];
                match (got.has(*c), e.y.has(*c)) {
                    (true, true) => k.tp += 1,
                    (true, false) => k.fp += 1,
                    (false, false) => k.tn += 1,
                    (false, true) => k.fn_ += 1,
                }
            }
        }
        if !data.is_empty() {
            out.exact_bp = (exact * 10_000 / data.len() as u64) as u32;
        }
        out
    }
}

/// The baseline every accuracy is reported next to: for one condition, the
/// best rule of the form "feature f >= t" (or "< t") chosen on `train`,
/// measured on `test`. Returns (feature, threshold, fires_above, test
/// accuracy in basis points).
pub fn one_rule(c: Condition, train: &[Example], test: &[Example]) -> (usize, i32, bool, u32) {
    let acc = |data: &[Example], f: usize, t: i32, above: bool| -> u64 {
        data.iter()
            .filter(|e| ((e.x[f] >= t) == above) == e.y.has(c))
            .count() as u64
    };
    let mut best = (0usize, 0i32, true, 0u64);
    for f in 0..FEATURES {
        for t in (0..=FEATURE_MAX).step_by(10) {
            for above in [true, false] {
                let a = acc(train, f, t, above);
                if a > best.3 {
                    best = (f, t, above, a);
                }
            }
        }
    }
    let test_bp = if test.is_empty() {
        0
    } else {
        (acc(test, best.0, best.1, best.2) * 10_000 / test.len() as u64) as u32
    };
    (best.0, best.1, best.2, test_bp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{parse, Scenario, MAX_SCENARIOS};

    const SCENARIOS: &str = include_str!("../../../ai/scenarios.txt");
    const PIN: &str = include_str!("../../../ai/diag.model.sha256");

    fn shipped() -> (Model, Vec<Example>, Vec<Example>) {
        let mut s = [Scenario::EMPTY; MAX_SCENARIOS];
        let n = parse(SCENARIOS, &mut s).expect("the shipped scenarios parse");
        let mut tr = vec![Example::ZERO; MAX_EXAMPLES];
        let mut te = vec![Example::ZERO; MAX_EXAMPLES];
        let (m, nt, ne) = train_from_scenarios(&s[..n], &SHIPPED, &mut tr, &mut te);
        tr.truncate(nt);
        te.truncate(ne);
        (m, tr, te)
    }

    fn hex(d: &[u8; 32]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn training_is_deterministic_and_pinned() {
        let (a, _, _) = shipped();
        let (b, _, _) = shipped();
        assert_eq!(encode(&a), encode(&b));
        // The pin: any change to the scenarios, the generator or the trainer
        // changes these bytes, and the change has to be made on purpose.
        let digest = hex(&crate::sha256::digest(&encode(&a)));
        assert_eq!(digest, PIN.trim(), "ai/diag.model.sha256 is stale");
    }

    #[test]
    fn the_shipped_model_is_measured_honestly() {
        let (m, tr, te) = shipped();
        let test = Metrics::of(&m, &te);
        let train = Metrics::of(&m, &tr);
        std::println!(
            "model exact-match train={} test={} bp (examples: {} train, {} test)",
            train.exact_bp,
            test.exact_bp,
            tr.len(),
            te.len()
        );
        for (i, c) in Condition::ALL.iter().enumerate() {
            let k = test.per_condition[i];
            let (f, t, above, base) = one_rule(*c, &tr, &te);
            std::println!(
                "  {}: test {} bp (tp={} fp={} tn={} fn={}); one-rule baseline feature {} {} {} -> {} bp",
                c.name(),
                k.accuracy_bp(),
                k.tp,
                k.fp,
                k.tn,
                k.fn_,
                f,
                if above { ">=" } else { "<" },
                t,
                base
            );
        }
        assert_eq!(m.test_exact_bp, test.exact_bp);
        assert!(test.exact_bp >= 9_500, "exact-match {} bp", test.exact_bp);
    }

    #[test]
    fn the_codec_round_trips_and_refuses_hostile_files() {
        let (m, _, _) = shipped();
        let b = encode(&m);
        assert_eq!(b.len(), LEN);
        assert_eq!(decode(&b), Ok(m));
        let bad = |f: &dyn Fn(&mut [u8; LEN])| {
            let mut x = b;
            f(&mut x);
            decode(&x)
        };
        assert_eq!(decode(&b[..LEN - 1]), Err(ModelError::BadLength));
        assert_eq!(decode(&[0u8; 0]), Err(ModelError::BadLength));
        assert_eq!(bad(&|x| x[0] = b'X'), Err(ModelError::BadMagic));
        assert_eq!(bad(&|x| x[8] = 17), Err(ModelError::BadDimensions));
        assert_eq!(bad(&|x| x[10] = 4), Err(ModelError::BadDimensions));
        assert_eq!(bad(&|x| x[36] = 1), Err(ModelError::BadReserved));
        assert_eq!(
            bad(&|x| x[32..36].copy_from_slice(&10_001u32.to_le_bytes())),
            Err(ModelError::BadAccuracy)
        );
        let at = HEADER_LEN + DETECTOR_LEN + 4 * 3;
        assert_eq!(
            bad(&|x| x[at..at + 4].copy_from_slice(&(MAX_WEIGHT + 1).to_le_bytes())),
            Err(ModelError::WeightOutOfRange)
        );
        assert_eq!(
            bad(&|x| x[at..at + 4].copy_from_slice(&i32::MIN.to_le_bytes())),
            Err(ModelError::WeightOutOfRange)
        );
    }

    #[test]
    fn detectors_are_independent_and_scores_cannot_overflow() {
        let mut d = [Detector::default(); CONDITIONS];
        d[0].bias = 1; // always fires
        d[1].bias = -1; // never fires
        d[2].weights[8] = 1; // fires on feature 8 > 0
        let m = Model {
            detectors: d,
            epochs: 0,
            seed: 0,
            train_examples: 0,
            test_examples: 0,
            test_exact_bp: 0,
            data_digest: [0; 32],
        };
        let mut x = [0i32; FEATURES];
        assert_eq!(
            m.detect(&x),
            Conditions::NONE.with(Condition::ServiceFailed)
        );
        x[8] = 1000;
        assert_eq!(
            m.detect(&x),
            Conditions::NONE
                .with(Condition::ServiceFailed)
                .with(Condition::DenialBurst)
        );
        // A score of exactly zero does not fire.
        let zero = Model {
            detectors: [Detector::default(); CONDITIONS],
            ..m
        };
        assert_eq!(zero.detect(&x), Conditions::NONE);
        // Extreme weights and hostile features stay inside i64.
        let big = Detector {
            bias: MAX_WEIGHT,
            weights: [MAX_WEIGHT; FEATURES],
        };
        assert_eq!(
            big.score(&[i32::MAX; FEATURES]),
            (1 + 16 * 1000) * i64::from(MAX_WEIGHT)
        );
        assert_eq!(big.score(&[i32::MIN; FEATURES]), i64::from(MAX_WEIGHT));
    }

    #[test]
    fn the_data_digest_covers_the_examples() {
        let (m, tr, te) = shipped();
        assert_eq!(m.data_digest, data_digest(&tr, &te));
        let mut te2 = te.clone();
        te2[0].x[0] ^= 1;
        assert_ne!(m.data_digest, data_digest(&tr, &te2));
        let mut te3 = te.clone();
        te3[0].y = Conditions(te3[0].y.0 ^ 1);
        assert_ne!(m.data_digest, data_digest(&tr, &te3));
    }
}
