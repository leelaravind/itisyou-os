//! The inference service's wire format (V0.11, INFER11-001, ADR-0024).
//!
//! `/bin/inferd` runs the diagnostic model in Ring 3 and answers over two IPC
//! channels: a request is a nonce and the 16 features of
//! [`crate::sysview::features`]; the reply echoes the nonce and carries the
//! conditions that fired, every detector's score and the SHA-256 of the
//! model that computed them. IPC carries no sender identity and any holder of
//! the IPC capability can read or write these channels, so a reply is a
//! CLAIM: the nonce only lets a client discard replies to someone else, and
//! the kernel recomputes a proposal's condition itself (ADR-0024 §7).
//!
//! ```text
//! request (72 bytes): nonce u64, 16 × i32 feature (each 0..=1000)
//! reply   (72 bytes): nonce u64, conditions u8, 7 × zero,
//!                     3 × i64 score (Condition::ALL order), model SHA-256
//! ```
//!
//! All integers little-endian. Both decoders are strict: exact length,
//! features in range, zero padding, and a conditions byte that agrees with
//! the scores (a bit is set exactly when its score is above zero).

use crate::model::{Model, CONDITIONS};
use crate::scenario::{Condition, Conditions};
use crate::sysview::{FEATURES, FEATURE_MAX};

/// Requests go to this channel ...
pub const REQUEST_CHANNEL: u64 = 6;
/// ... and replies come back on this one.
pub const REPLY_CHANNEL: u64 = 7;
pub const REQUEST_LEN: usize = 8 + 4 * FEATURES;
pub const REPLY_LEN: usize = 16 + 8 * CONDITIONS + 32;

/// A classification request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub nonce: u64,
    pub x: [i32; FEATURES],
}

/// inferd's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply {
    pub nonce: u64,
    pub conditions: Conditions,
    pub scores: [i64; CONDITIONS],
    pub model: [u8; 32],
}

/// Why a message was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    BadLength,
    FeatureOutOfRange,
    BadPadding,
    /// The conditions byte names a bit beyond the known conditions, or
    /// disagrees with the scores.
    Inconsistent,
}

impl WireError {
    pub const fn name(self) -> &'static str {
        match self {
            WireError::BadLength => "bad_length",
            WireError::FeatureOutOfRange => "feature_out_of_range",
            WireError::BadPadding => "bad_padding",
            WireError::Inconsistent => "inconsistent",
        }
    }
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

impl Request {
    pub fn encode(&self) -> [u8; REQUEST_LEN] {
        let mut b = [0u8; REQUEST_LEN];
        b[..8].copy_from_slice(&self.nonce.to_le_bytes());
        for (i, v) in self.x.iter().enumerate() {
            b[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Request, WireError> {
        if b.len() != REQUEST_LEN {
            return Err(WireError::BadLength);
        }
        let mut x = [0i32; FEATURES];
        for (i, v) in x.iter_mut().enumerate() {
            let o = 8 + 4 * i;
            *v = i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            if !(0..=FEATURE_MAX).contains(v) {
                return Err(WireError::FeatureOutOfRange);
            }
        }
        Ok(Request {
            nonce: u64_at(b, 0),
            x,
        })
    }
}

impl Reply {
    pub fn encode(&self) -> [u8; REPLY_LEN] {
        let mut b = [0u8; REPLY_LEN];
        b[..8].copy_from_slice(&self.nonce.to_le_bytes());
        b[8] = self.conditions.0;
        for (i, s) in self.scores.iter().enumerate() {
            b[16 + 8 * i..24 + 8 * i].copy_from_slice(&s.to_le_bytes());
        }
        b[16 + 8 * CONDITIONS..].copy_from_slice(&self.model);
        b
    }

    pub fn decode(b: &[u8]) -> Result<Reply, WireError> {
        if b.len() != REPLY_LEN {
            return Err(WireError::BadLength);
        }
        if b[9..16].iter().any(|&x| x != 0) {
            return Err(WireError::BadPadding);
        }
        let mut scores = [0i64; CONDITIONS];
        for (i, s) in scores.iter_mut().enumerate() {
            *s = u64_at(b, 16 + 8 * i) as i64;
        }
        let conditions = Conditions(b[8]);
        let mut expect = Conditions::NONE;
        for (c, s) in Condition::ALL.iter().zip(scores) {
            if s > 0 {
                expect = expect.with(*c);
            }
        }
        if conditions != expect {
            return Err(WireError::Inconsistent);
        }
        let mut model = [0u8; 32];
        model.copy_from_slice(&b[16 + 8 * CONDITIONS..]);
        Ok(Reply {
            nonce: u64_at(b, 0),
            conditions,
            scores,
            model,
        })
    }
}

/// What inferd answers: the model's scores and the conditions they fire.
pub fn answer(model: &Model, model_digest: &[u8; 32], req: &Request) -> Reply {
    Reply {
        nonce: req.nonce,
        conditions: model.detect(&req.x),
        scores: model.scores(&req.x),
        model: *model_digest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Detector, Model};

    fn model() -> Model {
        let mut d = [Detector::default(); CONDITIONS];
        d[0].weights[6] = 1; // service_failed: feature 6 > 0
        d[0].bias = -1;
        d[1].weights[8] = 1; // scheduler_paused: feature 8 > 0
        d[1].bias = -1;
        d[2].bias = -1; // denial_burst: never
        Model {
            detectors: d,
            epochs: 0,
            seed: 0,
            train_examples: 0,
            test_examples: 0,
            test_exact_bp: 0,
            data_digest: [0; 32],
        }
    }

    #[test]
    fn a_request_and_its_answer_round_trip() {
        let mut x = [0i32; FEATURES];
        x[6] = 500;
        let req = Request {
            nonce: 0xDEAD_BEEF_0000_0001,
            x,
        };
        assert_eq!(Request::decode(&req.encode()), Ok(req));
        let digest = [7u8; 32];
        let rep = answer(&model(), &digest, &req);
        assert_eq!(rep.nonce, req.nonce);
        assert_eq!(
            rep.conditions,
            Conditions::NONE.with(Condition::ServiceFailed)
        );
        assert_eq!(rep.scores, [499, -1, -1]);
        assert_eq!(Reply::decode(&rep.encode()), Ok(rep));
        assert_eq!(REQUEST_LEN, 72);
        assert_eq!(REPLY_LEN, 72);
    }

    #[test]
    fn hostile_messages_are_refused_by_name() {
        let req = Request {
            nonce: 1,
            x: [0; FEATURES],
        }
        .encode();
        assert_eq!(Request::decode(&req[..71]), Err(WireError::BadLength));
        let mut bad = req;
        bad[8..12].copy_from_slice(&1001i32.to_le_bytes());
        assert_eq!(Request::decode(&bad), Err(WireError::FeatureOutOfRange));
        bad[8..12].copy_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(Request::decode(&bad), Err(WireError::FeatureOutOfRange));

        let rep = answer(
            &model(),
            &[0; 32],
            &Request {
                nonce: 1,
                x: [0; FEATURES],
            },
        )
        .encode();
        assert_eq!(Reply::decode(&rep[..70]), Err(WireError::BadLength));
        let mut bad = rep;
        bad[12] = 1;
        assert_eq!(Reply::decode(&bad), Err(WireError::BadPadding));
        // A conditions byte that claims what the scores do not say.
        let mut bad = rep;
        bad[8] = Condition::DenialBurst.bit();
        assert_eq!(Reply::decode(&bad), Err(WireError::Inconsistent));
        let mut bad = rep;
        bad[8] = 0x80;
        assert_eq!(Reply::decode(&bad), Err(WireError::Inconsistent));
        // A score above zero without its bit.
        let mut bad = rep;
        bad[16..24].copy_from_slice(&5i64.to_le_bytes());
        assert_eq!(Reply::decode(&bad), Err(WireError::Inconsistent));
    }
}
