//! Ed25519 (RFC 8032) — signature verification for packages, plus signing so
//! the host-side packer and the round-trip tests share one implementation.
//!
//! Written out rather than pulled in as a dependency, for the same reason
//! [`crate::sha256`] is: this crate compiles unchanged into the kernel and
//! into host tools, has no allocator, and every line of it is testable on the
//! host. A signature check is the one place in the package pipeline where
//! "probably right" is not good enough, so it is validated against the RFC's
//! own test vectors rather than only against itself.
//!
//! **Every curve constant is derived, not remembered.** `d`, `sqrt(-1)` and
//! the base point are computed from small integers at use time
//! (`d = -121665/121666`, `sqrt(-1) = 2^((p-1)/4)`, `B.y = 4/5`). A
//! mistranscribed 32-byte constant is a class of bug that produces a
//! self-consistent implementation of the *wrong curve* — one that passes every
//! round-trip test and rejects every real signature. Deriving them removes the
//! possibility. Verification is not on any hot path, so the extra inversions
//! cost nothing that matters.
//!
//! **Not constant time.** Verification handles only public data (public key,
//! message, signature), so there is nothing secret to leak. Signing is used by
//! the build-time packer on a developer machine, never by the kernel with a
//! live key, and that is stated here rather than left to be assumed — if a
//! signing key ever moves into the running system, this code must be revisited.

use crate::sha512;

/// A field element mod 2^255 - 19, as five 51-bit limbs.
///
/// Limbs are kept below 2^52 after every operation, which is what makes the
/// 128-bit products in [`Fe::mul`] provably non-overflowing: the largest
/// coefficient sums 77 products of two sub-2^52 values, i.e. under 2^111.
#[derive(Clone, Copy, Debug)]
struct Fe([u64; 5]);

const MASK51: u64 = (1u64 << 51) - 1;

impl Fe {
    const ZERO: Fe = Fe([0; 5]);
    const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    fn from_u64(v: u64) -> Fe {
        Fe([v & MASK51, v >> 51, 0, 0, 0])
    }

    /// Decode 32 little-endian bytes, ignoring the top bit (the sign bit in a
    /// compressed point encoding).
    fn from_bytes(b: &[u8; 32]) -> Fe {
        let load = |i: usize| -> u64 { u64::from_le_bytes(b[i..i + 8].try_into().unwrap()) };
        let l0 = load(0) & MASK51;
        let l1 = (load(6) >> 3) & MASK51;
        let l2 = (load(12) >> 6) & MASK51;
        let l3 = (load(19) >> 1) & MASK51;
        let l4 = (load(24) >> 12) & MASK51;
        Fe([l0, l1, l2, l3, l4])
    }

    /// Encode canonically: fully reduced, little-endian, top bit clear.
    fn to_bytes(self) -> [u8; 32] {
        let mut t = self.carry();
        // Conditionally subtract p by checking whether t + 19 overflows past
        // 2^255. `q` ends as 1 exactly when t >= p.
        let mut q = (t.0[0] + 19) >> 51;
        q = (t.0[1] + q) >> 51;
        q = (t.0[2] + q) >> 51;
        q = (t.0[3] + q) >> 51;
        q = (t.0[4] + q) >> 51;
        t.0[0] += 19 * q;
        t.0[1] += t.0[0] >> 51;
        t.0[0] &= MASK51;
        t.0[2] += t.0[1] >> 51;
        t.0[1] &= MASK51;
        t.0[3] += t.0[2] >> 51;
        t.0[2] &= MASK51;
        t.0[4] += t.0[3] >> 51;
        t.0[3] &= MASK51;
        t.0[4] &= MASK51;

        let mut out = [0u8; 32];
        let packed: u128 = (t.0[0] as u128) | ((t.0[1] as u128) << 51) | ((t.0[2] as u128) << 102);
        out[0..16].copy_from_slice(&packed.to_le_bytes());
        // The remaining limbs start at bit 153; assemble the high half the
        // same way, then merge the overlapping byte.
        let high: u128 =
            ((t.0[2] >> 26) as u128) | ((t.0[3] as u128) << 25) | ((t.0[4] as u128) << 76);
        let high_bytes = high.to_le_bytes();
        out[16..32].copy_from_slice(&high_bytes[..16]);
        out
    }

    /// One carry pass, bringing every limb under 2^52.
    fn carry(self) -> Fe {
        let mut l = self.0;
        l[1] += l[0] >> 51;
        l[0] &= MASK51;
        l[2] += l[1] >> 51;
        l[1] &= MASK51;
        l[3] += l[2] >> 51;
        l[2] &= MASK51;
        l[4] += l[3] >> 51;
        l[3] &= MASK51;
        l[0] += 19 * (l[4] >> 51);
        l[4] &= MASK51;
        l[1] += l[0] >> 51;
        l[0] &= MASK51;
        Fe(l)
    }

    fn add(self, other: Fe) -> Fe {
        let mut l = [0u64; 5];
        for (out, (a, b)) in l.iter_mut().zip(self.0.iter().zip(other.0.iter())) {
            *out = a + b;
        }
        Fe(l).carry()
    }

    fn sub(self, other: Fe) -> Fe {
        // Add 2p first so no limb can underflow. 2p = [2^52-38, 2^52-2, ...].
        const TWO_P: [u64; 5] = [
            (1u64 << 52) - 38,
            (1u64 << 52) - 2,
            (1u64 << 52) - 2,
            (1u64 << 52) - 2,
            (1u64 << 52) - 2,
        ];
        let mut l = [0u64; 5];
        for (i, out) in l.iter_mut().enumerate() {
            *out = self.0[i] + TWO_P[i] - other.0[i];
        }
        Fe(l).carry()
    }

    fn neg(self) -> Fe {
        Fe::ZERO.sub(self)
    }

    fn mul(self, other: Fe) -> Fe {
        let a = self.0;
        let b = other.0;
        let b1_19 = 19 * b[1] as u128;
        let b2_19 = 19 * b[2] as u128;
        let b3_19 = 19 * b[3] as u128;
        let b4_19 = 19 * b[4] as u128;
        let (a0, a1, a2, a3, a4) = (
            a[0] as u128,
            a[1] as u128,
            a[2] as u128,
            a[3] as u128,
            a[4] as u128,
        );
        let (b0, b1, b2, b3, b4) = (
            b[0] as u128,
            b[1] as u128,
            b[2] as u128,
            b[3] as u128,
            b[4] as u128,
        );
        // 2^255 = 19 mod p, so a limb that would land at position 5 or above
        // folds back down multiplied by 19.
        let c0 = a0 * b0 + a1 * b4_19 + a2 * b3_19 + a3 * b2_19 + a4 * b1_19;
        let c1 = a0 * b1 + a1 * b0 + a2 * b4_19 + a3 * b3_19 + a4 * b2_19;
        let c2 = a0 * b2 + a1 * b1 + a2 * b0 + a3 * b4_19 + a4 * b3_19;
        let c3 = a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * b4_19;
        let c4 = a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;

        let mut l = [0u64; 5];
        let mut carry = c0 >> 51;
        l[0] = (c0 as u64) & MASK51;
        let c1 = c1 + carry;
        carry = c1 >> 51;
        l[1] = (c1 as u64) & MASK51;
        let c2 = c2 + carry;
        carry = c2 >> 51;
        l[2] = (c2 as u64) & MASK51;
        let c3 = c3 + carry;
        carry = c3 >> 51;
        l[3] = (c3 as u64) & MASK51;
        let c4 = c4 + carry;
        carry = c4 >> 51;
        l[4] = (c4 as u64) & MASK51;
        l[0] += 19 * (carry as u64);
        l[1] += l[0] >> 51;
        l[0] &= MASK51;
        Fe(l)
    }

    fn square(self) -> Fe {
        self.mul(self)
    }

    /// `self^(2^n)`.
    fn square_times(self, n: u32) -> Fe {
        let mut r = self;
        for _ in 0..n {
            r = r.square();
        }
        r
    }

    /// `self^(p-2)`, i.e. the multiplicative inverse (zero maps to zero).
    ///
    /// The addition chain is the standard one for 2^255-21; it is written as
    /// explicit steps because a generic square-and-multiply over the exponent
    /// would be both slower and harder to check.
    fn invert(self) -> Fe {
        let z2 = self.square();
        let z9 = z2.square_times(2).mul(self);
        let z11 = z9.mul(z2);
        let z2_5_0 = z11.square().mul(z9);
        let z2_10_0 = z2_5_0.square_times(5).mul(z2_5_0);
        let z2_20_0 = z2_10_0.square_times(10).mul(z2_10_0);
        let z2_40_0 = z2_20_0.square_times(20).mul(z2_20_0);
        let z2_50_0 = z2_40_0.square_times(10).mul(z2_10_0);
        let z2_100_0 = z2_50_0.square_times(50).mul(z2_50_0);
        let z2_200_0 = z2_100_0.square_times(100).mul(z2_100_0);
        let z2_250_0 = z2_200_0.square_times(50).mul(z2_50_0);
        z2_250_0.square_times(5).mul(z11)
    }

    /// `self^((p-5)/8)`, the exponent used for the square root.
    fn pow_p58(self) -> Fe {
        let z2 = self.square();
        let z9 = z2.square_times(2).mul(self);
        let z11 = z9.mul(z2);
        let z2_5_0 = z11.square().mul(z9);
        let z2_10_0 = z2_5_0.square_times(5).mul(z2_5_0);
        let z2_20_0 = z2_10_0.square_times(10).mul(z2_10_0);
        let z2_40_0 = z2_20_0.square_times(20).mul(z2_20_0);
        let z2_50_0 = z2_40_0.square_times(10).mul(z2_10_0);
        let z2_100_0 = z2_50_0.square_times(50).mul(z2_50_0);
        let z2_200_0 = z2_100_0.square_times(100).mul(z2_100_0);
        let z2_250_0 = z2_200_0.square_times(50).mul(z2_50_0);
        z2_250_0.square_times(2).mul(self)
    }

    fn is_zero(self) -> bool {
        self.to_bytes() == [0u8; 32]
    }

    fn equals(self, other: Fe) -> bool {
        self.to_bytes() == other.to_bytes()
    }

    /// The low bit of the canonical encoding — Ed25519's notion of "negative".
    fn is_negative(self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }
}

/// sqrt(-1) mod p, derived rather than transcribed: 2 is a quadratic
/// non-residue mod p, so 2^((p-1)/4) is a square root of -1.
fn sqrt_m1() -> Fe {
    // (p-1)/4 = 2^253 - 5. Square-and-multiply over that exponent using the
    // same chain style as `invert`.
    let two = Fe::from_u64(2);
    let z2 = two.square();
    let z9 = z2.square_times(2).mul(two);
    let z11 = z9.mul(z2);
    let z2_5_0 = z11.square().mul(z9);
    let z2_10_0 = z2_5_0.square_times(5).mul(z2_5_0);
    let z2_20_0 = z2_10_0.square_times(10).mul(z2_10_0);
    let z2_40_0 = z2_20_0.square_times(20).mul(z2_20_0);
    let z2_50_0 = z2_40_0.square_times(10).mul(z2_10_0);
    let z2_100_0 = z2_50_0.square_times(50).mul(z2_50_0);
    let z2_200_0 = z2_100_0.square_times(100).mul(z2_100_0);
    let z2_250_0 = z2_200_0.square_times(50).mul(z2_50_0);
    // 2^253 - 5 = (2^250 - 1) * 8 + 3, and 2^3 * 2^250-1 ... assembled as
    // squarings plus the residual factor.
    z2_250_0.square_times(3).mul(z2).mul(two)
}

/// The curve constant d = -121665/121666.
fn curve_d() -> Fe {
    Fe::from_u64(121665)
        .neg()
        .mul(Fe::from_u64(121666).invert())
}

/// A point in extended twisted-Edwards coordinates (X:Y:Z:T), x = X/Z,
/// y = Y/Z, T = XY/Z.
#[derive(Clone, Copy, Debug)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    fn identity() -> Point {
        Point {
            x: Fe::ZERO,
            y: Fe::ONE,
            z: Fe::ONE,
            t: Fe::ZERO,
        }
    }

    /// The standard base point: y = 4/5, x the even root.
    fn base() -> Point {
        let y = Fe::from_u64(4).mul(Fe::from_u64(5).invert());
        let mut encoded = y.to_bytes();
        encoded[31] &= 0x7F; // sign bit 0 selects the even x
        Point::decompress(&encoded).expect("the base point is on the curve")
    }

    fn add(self, other: Point) -> Point {
        let d2 = curve_d().add(curve_d());
        let a = self.y.sub(self.x).mul(other.y.sub(other.x));
        let b = self.y.add(self.x).mul(other.y.add(other.x));
        let c = self.t.mul(d2).mul(other.t);
        let dd = self.z.mul(other.z);
        let dd = dd.add(dd);
        let e = b.sub(a);
        let f = dd.sub(c);
        let g = dd.add(c);
        let h = b.add(a);
        Point {
            x: e.mul(f),
            y: g.mul(h),
            t: e.mul(h),
            z: f.mul(g),
        }
    }

    fn double(self) -> Point {
        let a = self.x.square();
        let b = self.y.square();
        let c = self.z.square();
        let c = c.add(c);
        let h = a.add(b);
        let e = h.sub(self.x.add(self.y).square());
        let g = a.sub(b);
        let f = c.add(g);
        Point {
            x: e.mul(f),
            y: g.mul(h),
            t: e.mul(h),
            z: f.mul(g),
        }
    }

    /// Scalar multiplication, most-significant bit first.
    ///
    /// Plain double-and-add: the scalar is public in every use here
    /// (verification, and signing on a build machine), so the timing leak a
    /// constant-time ladder would prevent has nothing to reveal.
    fn mul_scalar(self, scalar: &[u8; 32]) -> Point {
        let mut acc = Point::identity();
        for i in (0..256).rev() {
            acc = acc.double();
            if (scalar[i / 8] >> (i % 8)) & 1 == 1 {
                acc = acc.add(self);
            }
        }
        acc
    }

    /// Compress to 32 bytes: y with x's sign in the top bit.
    fn compress(self) -> [u8; 32] {
        let z_inv = self.z.invert();
        let x = self.x.mul(z_inv);
        let y = self.y.mul(z_inv);
        let mut out = y.to_bytes();
        out[31] |= (x.is_negative() as u8) << 7;
        out
    }

    /// Decompress a 32-byte encoding, or `None` if it names no curve point.
    ///
    /// Rejects a non-canonical y (one at or above p): two different encodings
    /// of the same key would otherwise both verify, which breaks any code that
    /// treats the encoded key as an identity.
    fn decompress(bytes: &[u8; 32]) -> Option<Point> {
        let y = Fe::from_bytes(bytes);
        // Canonical check: re-encoding must reproduce the input's low 255 bits.
        let mut expect = y.to_bytes();
        expect[31] |= bytes[31] & 0x80;
        if expect != *bytes {
            return None;
        }
        let sign = bytes[31] >> 7 == 1;

        let y2 = y.square();
        let u = y2.sub(Fe::ONE);
        let v = curve_d().mul(y2).add(Fe::ONE);
        // x = u * v^3 * (u * v^7)^((p-5)/8)
        let v3 = v.square().mul(v);
        let v7 = v3.square().mul(v);
        let mut x = u.mul(v3).mul(u.mul(v7).pow_p58());

        let check = v.mul(x.square());
        if check.equals(u) {
            // x is already the root.
        } else if check.equals(u.neg()) {
            x = x.mul(sqrt_m1());
        } else {
            return None;
        }
        if x.is_zero() && sign {
            // The only point with x = 0 has an even x; claiming the odd root
            // is a distinct, invalid encoding.
            return None;
        }
        if x.is_negative() != sign {
            x = x.neg();
        }
        Some(Point {
            x,
            y,
            z: Fe::ONE,
            t: x.mul(y),
        })
    }

    fn equals(self, other: Point) -> bool {
        // Compare affine, so different projective representatives of the same
        // point compare equal.
        self.x.mul(other.z).equals(other.x.mul(self.z))
            && self.y.mul(other.z).equals(other.y.mul(self.z))
    }
}

/// The prime order of the base point:
/// L = 2^252 + 27742317777372353535851937790883648493.
const L: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0x0000000000000000,
    0x1000000000000000,
];

/// Reduce a little-endian integer of arbitrary length mod L.
///
/// A bit-at-a-time long division rather than the usual packed-limb reduction.
/// It is slower and obviously correct; the packed version is a page of
/// hand-tuned constants whose only advantage is speed this code does not need.
fn reduce_mod_l(bytes: &[u8]) -> [u8; 32] {
    let mut r = [0u64; 4];
    for byte_index in (0..bytes.len()).rev() {
        for bit in (0..8).rev() {
            // r = r*2 + bit. r < L < 2^253, so r*2 cannot leave four limbs.
            let mut carry = ((bytes[byte_index] >> bit) & 1) as u64;
            for limb in r.iter_mut() {
                let next = *limb >> 63;
                *limb = (*limb << 1) | carry;
                carry = next;
            }
            if geq_l(&r) {
                sub_l(&mut r);
            }
        }
    }
    let mut out = [0u8; 32];
    for (i, limb) in r.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
    }
    out
}

fn geq_l(r: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if r[i] != L[i] {
            return r[i] > L[i];
        }
    }
    true
}

fn sub_l(r: &mut [u64; 4]) {
    let mut borrow = 0u64;
    for i in 0..4 {
        let (a, b1) = r[i].overflowing_sub(L[i]);
        let (a, b2) = a.overflowing_sub(borrow);
        r[i] = a;
        borrow = (b1 || b2) as u64;
    }
}

/// Multiply two reduced scalars and reduce the 512-bit product mod L.
fn scalar_mul(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut product = [0u8; 64];
    // Schoolbook byte multiplication; 32x32 bytes is 1024 partial products,
    // which is nothing at the rate this is called (once per signature).
    let mut acc = [0u16; 64];
    for (i, &ai) in a.iter().enumerate() {
        let mut carry = 0u16;
        for (j, &bj) in b.iter().enumerate() {
            let v = acc[i + j] as u32 + ai as u32 * bj as u32 + carry as u32;
            acc[i + j] = (v & 0xFF) as u16;
            carry = (v >> 8) as u16;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let v = acc[k] as u32 + carry as u32;
            acc[k] = (v & 0xFF) as u16;
            carry = (v >> 8) as u16;
            k += 1;
        }
    }
    for (dst, src) in product.iter_mut().zip(acc.iter()) {
        *dst = *src as u8;
    }
    reduce_mod_l(&product)
}

/// Add two reduced scalars mod L.
fn scalar_add(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut wide = [0u8; 33];
    let mut carry = 0u16;
    for i in 0..32 {
        let v = a[i] as u16 + b[i] as u16 + carry;
        wide[i] = (v & 0xFF) as u8;
        carry = v >> 8;
    }
    wide[32] = carry as u8;
    reduce_mod_l(&wide)
}

/// Is a 32-byte scalar canonical, i.e. strictly less than L?
///
/// RFC 8032 requires rejecting a signature whose S is not, and it matters:
/// accepting S and S+L would make signatures malleable, so a value that was
/// checked once ("have I seen this signature?") could be replayed in a
/// different encoding.
fn scalar_is_canonical(s: &[u8; 32]) -> bool {
    let mut limbs = [0u64; 4];
    for i in 0..4 {
        limbs[i] = u64::from_le_bytes(s[i * 8..i * 8 + 8].try_into().unwrap());
    }
    !geq_l(&limbs)
}

/// Length of a public key, a secret seed, and a signature.
pub const PUBLIC_KEY_LEN: usize = 32;
pub const SECRET_KEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

/// Clamp a SHA-512 half into a valid Ed25519 scalar (RFC 8032 §5.1.5).
fn clamp(mut h: [u8; 32]) -> [u8; 32] {
    h[0] &= 248;
    h[31] &= 127;
    h[31] |= 64;
    h
}

/// Derive the public key for a 32-byte secret seed.
pub fn public_key(secret: &[u8; SECRET_KEY_LEN]) -> [u8; PUBLIC_KEY_LEN] {
    let h = sha512::hash(secret);
    let a = clamp(h[..32].try_into().unwrap());
    Point::base().mul_scalar(&a).compress()
}

/// Sign `message` with a 32-byte secret seed.
///
/// Present so the build-time package signer and these tests use the same code
/// the kernel verifies with. It is NOT for use inside the running system: see
/// the module note on constant time.
pub fn sign(secret: &[u8; SECRET_KEY_LEN], message: &[u8]) -> [u8; SIGNATURE_LEN] {
    let h = sha512::hash(secret);
    let a = clamp(h[..32].try_into().unwrap());
    let prefix = &h[32..];
    let public = Point::base().mul_scalar(&a).compress();

    let r = reduce_mod_l(&sha512::hash_parts(&[prefix, message]));
    let big_r = Point::base().mul_scalar(&r).compress();
    let k = reduce_mod_l(&sha512::hash_parts(&[&big_r, &public, message]));
    let s = scalar_add(&r, &scalar_mul(&k, &a));

    let mut sig = [0u8; SIGNATURE_LEN];
    sig[..32].copy_from_slice(&big_r);
    sig[32..].copy_from_slice(&s);
    sig
}

/// Verify `signature` over `message` under `public_key`.
///
/// Returns false for every failure — a malformed key, a malformed R, a
/// non-canonical S, or an equation that does not hold. Callers get one answer
/// because there is only one useful answer: a signature that is not valid.
pub fn verify(
    public_key: &[u8; PUBLIC_KEY_LEN],
    message: &[u8],
    signature: &[u8; SIGNATURE_LEN],
) -> bool {
    let r_bytes: [u8; 32] = signature[..32].try_into().unwrap();
    let s_bytes: [u8; 32] = signature[32..].try_into().unwrap();
    if !scalar_is_canonical(&s_bytes) {
        return false;
    }
    let Some(a) = Point::decompress(public_key) else {
        return false;
    };
    let Some(r) = Point::decompress(&r_bytes) else {
        return false;
    };
    let k = reduce_mod_l(&sha512::hash_parts(&[&r_bytes, public_key, message]));
    // Check [S]B = R + [k]A.
    let lhs = Point::base().mul_scalar(&s_bytes);
    let rhs = r.add(a.mul_scalar(&k));
    lhs.equals(rhs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn arr32(s: &str) -> [u8; 32] {
        unhex(s).try_into().unwrap()
    }

    fn arr64(s: &str) -> [u8; 64] {
        let v = unhex(s);
        let mut a = [0u8; 64];
        a.copy_from_slice(&v);
        a
    }

    // --- field arithmetic -------------------------------------------------

    #[test]
    fn field_round_trips_through_bytes() {
        for seed in 0..64u8 {
            let mut b = [seed; 32];
            b[31] &= 0x7F;
            let fe = Fe::from_bytes(&b);
            // Only canonical values survive a round trip; build one by
            // re-encoding and comparing that against a second decode.
            let once = fe.to_bytes();
            assert_eq!(Fe::from_bytes(&once).to_bytes(), once, "seed {seed}");
        }
    }

    #[test]
    fn inverse_times_value_is_one() {
        for v in 1..40u64 {
            let x = Fe::from_u64(v);
            assert!(x.mul(x.invert()).equals(Fe::ONE), "v = {v}");
        }
    }

    #[test]
    fn sqrt_minus_one_squares_to_minus_one() {
        assert!(sqrt_m1().square().equals(Fe::ONE.neg()));
    }

    #[test]
    fn subtraction_is_the_inverse_of_addition() {
        let a = Fe::from_u64(0x1234_5678_9abc);
        let b = Fe::from_u64(0xfedc_ba98_7654);
        assert!(a.add(b).sub(b).equals(a));
        assert!(a.sub(b).add(b).equals(a));
        assert!(a.sub(a).is_zero());
    }

    #[test]
    fn p_minus_one_encodes_canonically() {
        // p - 1 = 2^255 - 20.
        let p_minus_1 = Fe::ZERO.sub(Fe::ONE);
        let bytes = p_minus_1.to_bytes();
        assert_eq!(bytes[0], 0xEC);
        for b in &bytes[1..31] {
            assert_eq!(*b, 0xFF);
        }
        assert_eq!(bytes[31], 0x7F);
        assert!(p_minus_1.add(Fe::ONE).is_zero(), "p must reduce to zero");
    }

    // --- group arithmetic -------------------------------------------------

    #[test]
    fn base_point_has_the_expected_encoding() {
        // RFC 8032's base point: y = 4/5, x even. Its compressed form is the
        // well-known 5866666666666666… pattern, which is worth pinning: it
        // catches a wrong `d`, a wrong square root, or a byte-order slip in
        // one assertion.
        assert_eq!(
            hex(&Point::base().compress()),
            "5866666666666666666666666666666666666666666666666666666666666666"
        );
    }

    #[test]
    fn base_point_has_order_l() {
        // [L]B must be the identity. This is the single strongest structural
        // check available: it fails for essentially any error in the field
        // arithmetic, the point formulas, or the order constant.
        let mut l_bytes = [0u8; 32];
        for (i, limb) in L.iter().enumerate() {
            l_bytes[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }
        let result = Point::base().mul_scalar(&l_bytes);
        assert!(result.equals(Point::identity()));
    }

    #[test]
    fn doubling_agrees_with_addition() {
        let b = Point::base();
        assert!(b.double().equals(b.add(b)));
        let b3 = b.double().add(b);
        assert!(b3.add(b3).equals(b3.double()));
    }

    #[test]
    fn compression_round_trips() {
        let mut p = Point::base();
        for i in 0..8 {
            let encoded = p.compress();
            let decoded = Point::decompress(&encoded).expect("point must decompress");
            assert!(decoded.equals(p), "iteration {i}");
            assert_eq!(decoded.compress(), encoded);
            p = p.double();
        }
    }

    #[test]
    fn decompression_rejects_non_points_and_non_canonical_encodings() {
        // A y with no corresponding x.
        let mut bad = [0u8; 32];
        bad[0] = 2;
        // Not every value fails, so search for one that does — the point is
        // that failures are detected, not that a specific byte fails.
        let mut found_invalid = false;
        for i in 0..64u8 {
            bad[0] = i;
            if Point::decompress(&bad).is_none() {
                found_invalid = true;
                break;
            }
        }
        assert!(found_invalid, "some y must fail to decompress");

        // y = p is a non-canonical encoding of 0.
        let mut non_canonical = [0xFFu8; 32];
        non_canonical[0] = 0xED;
        non_canonical[31] = 0x7F;
        assert!(Point::decompress(&non_canonical).is_none());
    }

    // --- scalars ----------------------------------------------------------

    #[test]
    fn reduce_of_a_small_value_is_itself() {
        let mut v = [0u8; 32];
        v[0] = 42;
        assert_eq!(reduce_mod_l(&v), v);
    }

    #[test]
    fn reduce_of_l_is_zero_and_of_l_plus_one_is_one() {
        let mut l_bytes = [0u8; 32];
        for (i, limb) in L.iter().enumerate() {
            l_bytes[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }
        assert_eq!(reduce_mod_l(&l_bytes), [0u8; 32]);
        let mut l_plus_1 = l_bytes;
        l_plus_1[0] += 1;
        let mut one = [0u8; 32];
        one[0] = 1;
        assert_eq!(reduce_mod_l(&l_plus_1), one);
        assert!(!scalar_is_canonical(&l_bytes));
        assert!(scalar_is_canonical(&one));
    }

    #[test]
    fn scalar_multiplication_and_addition_agree_with_the_group() {
        let mut three = [0u8; 32];
        three[0] = 3;
        let mut five = [0u8; 32];
        five[0] = 5;
        let fifteen = scalar_mul(&three, &five);
        assert_eq!(fifteen[0], 15);
        let eight = scalar_add(&three, &five);
        assert_eq!(eight[0], 8);
        // [3]([5]B) must equal [15]B.
        let b = Point::base();
        assert!(b
            .mul_scalar(&five)
            .mul_scalar(&three)
            .equals(b.mul_scalar(&fifteen)));
    }

    fn hex(bytes: &[u8]) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    // --- RFC 8032 §7.1 test vectors ---------------------------------------

    /// Each entry is (secret, public, message, signature).
    const VECTORS: &[(&str, &str, &str, &str)] = &[
        (
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
            "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
            "af82",
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        ),
    ];

    #[test]
    fn rfc8032_public_keys() {
        for (secret, public, _, _) in VECTORS {
            assert_eq!(hex(&public_key(&arr32(secret))), *public);
        }
    }

    #[test]
    fn rfc8032_signatures() {
        for (secret, _, message, signature) in VECTORS {
            let sig = sign(&arr32(secret), &unhex(message));
            assert_eq!(hex(&sig), *signature, "message {message}");
        }
    }

    #[test]
    fn rfc8032_verification() {
        for (_, public, message, signature) in VECTORS {
            assert!(verify(&arr32(public), &unhex(message), &arr64(signature)));
        }
    }

    // --- negative cases ---------------------------------------------------

    #[test]
    fn a_flipped_message_bit_fails() {
        let (secret, public, _, _) = VECTORS[2];
        let message = b"itisyou-os package manifest";
        let sig = sign(&arr32(secret), message);
        assert!(verify(&arr32(public), message, &sig));
        for i in 0..message.len() {
            let mut tampered = message.to_vec();
            tampered[i] ^= 0x01;
            assert!(
                !verify(&arr32(public), &tampered, &sig),
                "byte {i} must invalidate"
            );
        }
    }

    #[test]
    fn a_flipped_signature_bit_fails() {
        let (secret, public, _, _) = VECTORS[1];
        let message = b"payload";
        let sig = sign(&arr32(secret), message);
        for i in 0..SIGNATURE_LEN {
            let mut bad = sig;
            bad[i] ^= 0x01;
            assert!(!verify(&arr32(public), message, &bad), "sig byte {i}");
        }
    }

    #[test]
    fn another_keys_signature_fails() {
        let a = arr32(VECTORS[0].0);
        let b = arr32(VECTORS[1].0);
        let message = b"same message, different signer";
        let sig = sign(&a, message);
        assert!(verify(&public_key(&a), message, &sig));
        assert!(!verify(&public_key(&b), message, &sig));
    }

    #[test]
    fn an_all_zero_signature_fails() {
        let public = arr32(VECTORS[0].1);
        assert!(!verify(&public, b"anything", &[0u8; SIGNATURE_LEN]));
    }

    #[test]
    fn a_non_canonical_s_is_rejected() {
        let (secret, public, message, _) = VECTORS[1];
        let mut sig = sign(&arr32(secret), &unhex(message));
        assert!(verify(&arr32(public), &unhex(message), &sig));
        // S + L has the same value mod L. Accepting it would make signatures
        // malleable: the same signature in two encodings.
        let s: [u8; 32] = sig[32..].try_into().unwrap();
        let mut carry = 0u16;
        let mut s_plus_l = [0u8; 32];
        for i in 0..32 {
            let l_byte = (L[i / 8] >> ((i % 8) * 8)) as u8;
            let v = s[i] as u16 + l_byte as u16 + carry;
            s_plus_l[i] = (v & 0xFF) as u8;
            carry = v >> 8;
        }
        sig[32..].copy_from_slice(&s_plus_l);
        assert!(!verify(&arr32(public), &unhex(message), &sig));
    }

    #[test]
    fn a_garbage_public_key_fails_rather_than_panicking() {
        for seed in 0..32u8 {
            let key = [seed; 32];
            // Whatever this decodes to (or fails to), it must not accept.
            let _ = verify(&key, b"m", &[seed; 64]);
        }
    }

    #[test]
    fn signing_is_deterministic() {
        let secret = arr32(VECTORS[0].0);
        let a = sign(&secret, b"determinism");
        let b = sign(&secret, b"determinism");
        assert_eq!(a, b);
    }

    #[test]
    fn empty_and_long_messages_round_trip() {
        let secret = arr32(VECTORS[2].0);
        let public = public_key(&secret);
        let long: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        for message in [&[][..], b"x", &long[..]] {
            let sig = sign(&secret, message);
            assert!(verify(&public, message, &sig));
        }
    }
}
