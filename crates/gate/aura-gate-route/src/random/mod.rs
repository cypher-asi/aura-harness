//! The random draws exploring takes, and the small generator Gate draws
//! them from.

/// Source of the random draws exploration needs.
pub trait Random {
    /// A number in `[0, 1)`.
    fn unit(&mut self) -> f64;
}

impl<F: FnMut() -> f64> Random for F {
    fn unit(&mut self) -> f64 {
        self()
    }
}

/// How far SplitMix64's state moves on each draw: 2^64 over the golden
/// ratio, made odd.
const STEP: u64 = 0x9E37_79B9_7F4A_7C15;

/// The multipliers SplitMix64 mixes its state with into a draw.
const MIX: [u64; 2] = [0xBF58_476D_1CE4_E5B9, 0x94D0_49BB_1331_11EB];

/// The random bits a draw keeps: as many as an `f64`'s mantissa holds.
const MANTISSA_BITS: u32 = f64::MANTISSA_DIGITS - 1;

/// A small, fast generator (SplitMix64); plenty for deciding when to
/// explore.
#[derive(Debug, Clone)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// A generator that always gives the same draws for `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator seeded from the clock.
    #[must_use]
    pub fn from_clock() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        // Only the low bits vary between runs; folding the high half in
        // keeps the whole value.
        let folded = (nanos ^ (nanos >> u64::BITS)) & u128::from(u64::MAX);
        Self(u64::try_from(folded).unwrap_or_default())
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(STEP);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(MIX[0]);
        z = (z ^ (z >> 27)).wrapping_mul(MIX[1]);
        z ^ (z >> 31)
    }
}

impl Random for SplitMix64 {
    fn unit(&mut self) -> f64 {
        // Random mantissa bits under the exponent of 1.0 give [1, 2).
        let mantissa = self.next_u64() >> (u64::BITS - MANTISSA_BITS);
        f64::from_bits(1.0_f64.to_bits() | mantissa) - 1.0
    }
}

#[cfg(test)]
mod tests;
