//! Temperature, then top-p, then a random draw: the order vLLM samples in, which the prompt contract's
//! numbers were chosen with.

/// A small PRNG (splitmix64).
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// Uniform in `0..1`.
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Draws a token among `allowed` (index, logit). `None` when nothing is allowed.
pub(crate) fn draw<I: Iterator<Item = (u32, f32)>>(
    allowed: impl Fn() -> I,
    temperature: f32,
    top_p: f32,
    rng: &mut Rng,
) -> Option<u32> {
    let mut best = None;
    for (id, l) in allowed() {
        if l.is_finite() && best.is_none_or(|(_, b)| l > b) {
            best = Some((id, l));
        }
    }
    let (best_id, max) = best?;
    if temperature <= 0.0 {
        return Some(best_id);
    }
    // Below e^-20 of the best token nothing can be in a top-p nucleus or be drawn.
    let floor = max - 20.0 * temperature;
    let mut probs: Vec<(u32, f32)> = allowed()
        .filter(|(_, l)| *l >= floor)
        .map(|(id, l)| (id, ((l - max) / temperature).exp()))
        .collect();
    probs.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    let total: f32 = probs.iter().map(|p| p.1).sum();
    let mut kept = 0.0;
    let mut n = 0;
    for p in &probs {
        kept += p.1;
        n += 1;
        if kept >= top_p * total {
            break;
        }
    }
    let mut at = rng.next() * kept;
    for p in &probs[..n] {
        at -= p.1;
        if at < 0.0 {
            return Some(p.0);
        }
    }
    Some(probs[n - 1].0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_from_the_nucleus() {
        let logits = [1.0f32, 5.0, 4.9, -3.0];
        let all = || (0u32..).zip(logits);
        let mut rng = Rng::new(1);
        assert_eq!(draw(all, 0.0, 1.0, &mut rng), Some(1));
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..500 {
            seen.insert(draw(all, 1.0, 0.8, &mut rng).unwrap());
        }
        assert_eq!(seen.into_iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(draw(std::iter::empty, 1.0, 0.8, &mut rng), None);
    }
}
