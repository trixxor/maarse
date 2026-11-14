//! Rutetabel med fast størrelse.
//!
//! Hver rute er en kildesti til en destination. Når tabellen er fuld,
//! smides den rute ud, der er brugt mindst for nylig (LRU).

use crate::addr::Address;
use crate::packet::Path;

/// En rute bliver betragtet som forældet efter denne tid (ms) og kan
/// overskrives af en dårligere rute.
pub const ROUTE_STALE_MS: u64 = 30 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub dest: Address,
    /// Repeatere fra os til destinationen.
    pub path: Path,
    /// Lavere er bedre. I v0.1 = antal hop.
    pub cost: u16,
    pub last_seen_ms: u64,
    /// Hvornår ruten blev fundet.
    pub learned_ms: u64,
}

pub struct RouteTable<const N: usize> {
    entries: heapless::Vec<Route, N>,
}

impl<const N: usize> Default for RouteTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RouteTable<N> {
    pub const fn new() -> Self {
        RouteTable {
            entries: heapless::Vec::new(),
        }
    }

    pub fn get(&self, dest: &Address) -> Option<&Route> {
        self.entries.iter().find(|r| r.dest == *dest)
    }

    /// Markerer en rute som brugt, så den ikke bliver smidt ud først.
    pub fn touch(&mut self, dest: &Address, now_ms: u64) {
        if let Some(r) = self.entries.iter_mut().find(|r| r.dest == *dest) {
            r.last_seen_ms = now_ms;
        }
    }

    /// Indsætter eller opdaterer en rute.
    ///
    /// En eksisterende rute erstattes, hvis den nye er mindst lige så god,
    /// eller hvis den gamle er forældet. Returnerer `true` hvis tabellen
    /// blev ændret.
    pub fn upsert(&mut self, route: Route) -> bool {
        if let Some(old) = self.entries.iter_mut().find(|r| r.dest == route.dest) {
            let stale = route.last_seen_ms.saturating_sub(old.last_seen_ms) > ROUTE_STALE_MS;
            if route.cost <= old.cost || stale {
                *old = route;
                return true;
            }
            return false;
        }
        if self.entries.is_full() {
            let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, r)| r.last_seen_ms)
                .map(|(i, _)| i)
            else {
                return false; // N == 0
            };
            self.entries.swap_remove(oldest);
        }
        self.entries.push(route).is_ok()
    }

    pub fn remove(&mut self, dest: &Address) -> Option<Route> {
        let i = self.entries.iter().position(|r| r.dest == *dest)?;
        Some(self.entries.swap_remove(i))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Route> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(d: u8, hops: &[u16], t: u64) -> Route {
        Route {
            dest: Address([d; 16]),
            path: Path::from_slice(hops).unwrap(),
            cost: hops.len() as u16,
            last_seen_ms: t,
            learned_ms: t,
        }
    }

    #[test]
    fn keeps_better_route() {
        let mut t = RouteTable::<4>::new();
        assert!(t.upsert(route(1, &[1, 2, 3], 0)));
        assert!(t.upsert(route(1, &[4], 10)));
        assert!(!t.upsert(route(1, &[5, 6], 20)));
        assert_eq!(t.get(&Address([1; 16])).unwrap().path.as_slice(), &[4]);
    }

    #[test]
    fn replaces_stale_route() {
        let mut t = RouteTable::<4>::new();
        t.upsert(route(1, &[4], 0));
        assert!(t.upsert(route(1, &[5, 6], ROUTE_STALE_MS + 1)));
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut t = RouteTable::<2>::new();
        t.upsert(route(1, &[], 0));
        t.upsert(route(2, &[], 5));
        t.touch(&Address([1; 16]), 10);
        t.upsert(route(3, &[], 20));
        assert!(t.get(&Address([1; 16])).is_some());
        assert!(t.get(&Address([2; 16])).is_none());
        assert_eq!(t.len(), 2);
    }
}
