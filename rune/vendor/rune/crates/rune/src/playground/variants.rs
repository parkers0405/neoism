//! Variants: a story's state made with one of its named tweaks, picked by 1 to 9.

use super::*;

impl<S: 'static> Live<S> {
    /// The open story's variants' names, in key order.
    pub(super) fn variants(&self) -> Vec<&'static str> {
        self.variants_of(self.selected)
    }

    fn variants_of(&self, i: usize) -> Vec<&'static str> {
        if i < self.scenes.len() {
            return self.variants.iter().filter(|(s, _, _)| *s == i).map(|(_, name, _)| *name).collect();
        }
        match self.pages.get(i - self.scenes.len()) {
            Some(Page { kind: Kind::Own(own), .. }) => own.variants(),
            _ => Vec::new(),
        }
    }

    /// `--variant`: story `name` (else the first) opens on the variant `variant` names, by
    /// number from 1 or by name.
    pub(super) fn pick(&mut self, name: Option<&str>, variant: &str) -> Result<(), String> {
        let i = name.and_then(|n| self.find(n)).unwrap_or(self.selected);
        let names = self.variants_of(i);
        let by_number = variant.parse::<usize>().ok().filter(|n| (1..=names.len()).contains(n)).map(|n| n - 1);
        let by_name = || names.iter().position(|n| *n == variant).or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(variant)));
        match by_number.or_else(by_name) {
            Some(n) => self.variant[i] = n,
            None if names.is_empty() => return Err(format!("{:?} has no variants", self.path(i))),
            None => return Err(format!("no variant {variant:?} of {:?}; it has: {}", self.path(i), names.join(", "))),
        }
        // A story's stage outlives a switch away; made again, it takes the variant.
        if let Some(page) = i.checked_sub(self.scenes.len()) && let Some(stage) = self.owned.get_mut(page) { *stage = None; }
        Ok(())
    }

    /// The open story's variant, by index, of those it has.
    pub(super) fn variant_at(&self) -> usize {
        self.variant.get(self.selected).copied().unwrap_or(0)
    }

    /// The open scene's variant's fn, if it has variants.
    pub(super) fn scene_variant(&self) -> Option<&dyn Fn(&mut S)> {
        self.variants.iter().filter(|(s, _, _)| *s == self.selected).nth(self.variant_at()).map(|(_, _, apply)| apply.as_ref())
    }

    /// What a digit key does: the open story's variant it names, if it has that many.
    pub(super) fn variant_key(&self, c: char) -> Option<Request> {
        let n = c.to_digit(10).filter(|d| (1..=9).contains(d))? as usize;
        (n <= self.variants().len()).then_some(Request::Variant(n - 1))
    }
}
