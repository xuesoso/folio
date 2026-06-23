//! A small LRU cache of protocol-encoded, display-ready page crops.
//!
//! Keyed by everything that affects the pixels — page, render width (zoom), and
//! the visible window — so revisiting a view is instant: no re-rasterize and no
//! re-encode (PNG/sixel compression is the expensive step). We cache the encoded
//! payload (~tens of KB) rather than raw RGBA, so memory stays flat regardless of
//! document length.

use std::collections::VecDeque;
use std::rc::Rc;

use crate::render::EncodedImage;

/// Identifies a cached render: a page at a zoom (render width), cropped to a
/// visible window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheKey {
    pub page: usize,
    pub render_w: u32,
    pub win_x: u32,
    pub win_y: u32,
    pub win_w: u32,
    pub win_h: u32,
    /// Color theme discriminant (different themes cache separately).
    pub theme: u8,
    /// View rotation in 90° clockwise turns (0..=3); rotated views cache apart.
    pub rotation: u8,
}

pub struct RenderCache {
    /// Front = least-recently-used, back = most-recently-used.
    lru: VecDeque<(CacheKey, Rc<EncodedImage>)>,
    budget_bytes: usize,
    used_bytes: usize,
}

impl RenderCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            lru: VecDeque::new(),
            budget_bytes,
            used_bytes: 0,
        }
    }

    pub fn get(&mut self, key: CacheKey) -> Option<Rc<EncodedImage>> {
        let pos = self.lru.iter().position(|(k, _)| *k == key)?;
        let entry = self.lru.remove(pos).expect("position just found");
        let img = entry.1.clone();
        self.lru.push_back(entry);
        Some(img)
    }

    pub fn insert(&mut self, key: CacheKey, img: EncodedImage) -> Rc<EncodedImage> {
        if let Some(pos) = self.lru.iter().position(|(k, _)| *k == key) {
            let (_, old) = self.lru.remove(pos).expect("position just found");
            self.used_bytes -= old.size();
        }
        let rc = Rc::new(img);
        self.used_bytes += rc.size();
        self.lru.push_back((key, rc.clone()));
        self.evict_to_budget();
        rc
    }

    #[cfg(test)]
    pub fn contains(&self, key: CacheKey) -> bool {
        self.lru.iter().any(|(k, _)| *k == key)
    }

    fn evict_to_budget(&mut self) {
        while self.used_bytes > self.budget_bytes && self.lru.len() > 1 {
            if let Some((_, img)) = self.lru.pop_front() {
                self.used_bytes -= img.size();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(bytes: usize) -> EncodedImage {
        EncodedImage {
            payload: vec![0u8; bytes],
        }
    }

    fn key(page: usize) -> CacheKey {
        CacheKey {
            page,
            render_w: 100,
            win_x: 0,
            win_y: 0,
            win_w: 100,
            win_h: 100,
            theme: 0,
            rotation: 0,
        }
    }

    #[test]
    fn hit_after_insert() {
        let mut c = RenderCache::new(10 * 1024 * 1024);
        c.insert(key(0), enc(64));
        assert!(c.get(key(0)).is_some());
        assert!(c.get(key(1)).is_none());
    }

    #[test]
    fn evicts_least_recently_used_past_budget() {
        let mut c = RenderCache::new(40);
        c.insert(key(0), enc(16));
        c.insert(key(1), enc(16));
        assert!(c.get(key(0)).is_some()); // page 1 becomes LRU
        c.insert(key(2), enc(16)); // 48 > 40 -> evict page 1
        assert!(c.contains(key(0)));
        assert!(c.contains(key(2)));
        assert!(!c.contains(key(1)));
    }
}
