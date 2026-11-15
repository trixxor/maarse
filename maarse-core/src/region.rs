//! Regions for channels and other floods.
//!
//! A region is a lowercase name, e.g. `dk` or `nordjylland`. On
//! the air it is carried as a 16-bit id (see [`region_id`]); 0 means no
//! region (global, as before).
//!
//! Every node has a list of up to [`MAX_REGIONS`] regions it serves.
//! Hierarchy is expressed by configuration: a repeater in Aalborg has
//! `["dk", "nordjylland"]`. A flood with a region is only forwarded by nodes
//! whose list contains the region. A node without regions forwards everything,
//! unless it is `strict` – then it only forwards floods without a region.
//! Reception for the node's own user is never filtered.

/// Largest number of regions per node.
pub const MAX_REGIONS: usize = 8;

pub type Regions = heapless::Vec<u16, MAX_REGIONS>;

/// 16-bit id for a region name: FNV-1a (32 bit) over
/// `"maarse-region:" + lowercase name`, folded to 16 bits
/// (high 16 bits XOR low 16 bits). An empty name gives 0; a hash that hits 0
/// becomes 1.
///
/// FNV instead of SHA-256, so the core stays free of crypto dependencies.
pub fn region_id(name: &str) -> u16 {
    let name = name.trim();
    if name.is_empty() {
        return 0;
    }
    let mut h: u32 = 0x811c_9dc5;
    let mut feed = |b: u8| {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    };
    b"maarse-region:".iter().copied().for_each(&mut feed);
    for c in name.chars() {
        for l in c.to_lowercase() {
            let mut b = [0u8; 4];
            l.encode_utf8(&mut b).bytes().for_each(&mut feed);
        }
    }
    match ((h >> 16) ^ h) as u16 {
        0 => 1,
        x => x,
    }
}

/// Splits a comma separated list (`"dk, nordjylland"`) into names.
pub fn names(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// Ids for a comma separated list (at most [`MAX_REGIONS`]).
pub fn parse_list(list: &str) -> Regions {
    let mut v = Regions::new();
    for n in names(list) {
        let id = region_id(n);
        if !v.contains(&id) && v.push(id).is_err() {
            break;
        }
    }
    v
}

/// Splits a channel name `name@region` into (name, region). Without `@` the region is empty.
pub fn split_channel(name: &str) -> (&str, &str) {
    match name.rsplit_once('@') {
        Some((n, r)) => (n.trim(), r.trim()),
        None => (name.trim(), ""),
    }
}

/// May a node with `regions` (and `strict`) forward a flood in `region`?
pub fn forwards(regions: &[u16], strict: bool, region: u16) -> bool {
    if region == 0 {
        return true;
    }
    if regions.is_empty() {
        return !strict;
    }
    regions.contains(&region)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_case_insensitive() {
        assert_eq!(region_id(""), 0);
        assert_ne!(region_id("dk"), 0);
        assert_eq!(region_id("DK"), region_id("dk"));
        assert_eq!(region_id(" dk "), region_id("dk"));
        assert_ne!(region_id("dk"), region_id("nordjylland"));
    }

    #[test]
    fn list_and_channel_parsing() {
        let l = parse_list("dk, nordjylland,,DK");
        assert_eq!(&l[..], &[region_id("dk"), region_id("nordjylland")]);
        assert_eq!(split_channel("#tur@nordjylland"), ("#tur", "nordjylland"));
        assert_eq!(split_channel("#tur"), ("#tur", ""));
    }

    #[test]
    fn forwarding_rule() {
        let dk = region_id("dk");
        let nj = region_id("nordjylland");
        // Without a region: always.
        assert!(forwards(&[], false, 0));
        assert!(forwards(&[], true, 0));
        assert!(forwards(&[dk], true, 0));
        // Empty list: everything, unless strict.
        assert!(forwards(&[], false, nj));
        assert!(!forwards(&[], true, nj));
        // List: only its own regions.
        assert!(forwards(&[dk, nj], false, nj));
        assert!(forwards(&[dk], true, dk));
        assert!(!forwards(&[dk], false, nj));
    }
}
