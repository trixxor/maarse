//! Regioner for kanaler og andre floods.
//!
//! En region er et navn med små bogstaver, fx `dk` eller `nordjylland`. På
//! luften bæres den som et 16-bit id (se [`region_id`]); 0 betyder uden
//! region (global, som før).
//!
//! Hver node har en liste med op til [`MAX_REGIONS`] regioner, den betjener.
//! Hierarki udtrykkes med konfiguration: en repeater i Aalborg har
//! `["dk", "nordjylland"]`. Et flood med region videresendes kun af noder,
//! hvis liste indeholder regionen. En node uden regioner videresender alt,
//! medmindre den er `strict` – så videresender den kun floods uden region.
//! Modtagelse til egen bruger filtreres aldrig.

/// Største antal regioner pr. node.
pub const MAX_REGIONS: usize = 8;

pub type Regions = heapless::Vec<u16, MAX_REGIONS>;

/// 16-bit id for et regionsnavn: FNV-1a (32 bit) over
/// `"maarse-region:" + navn med små bogstaver`, foldet til 16 bit
/// (høje 16 bit XOR lave 16 bit). Tomt navn giver 0; et hash, der rammer 0,
/// bliver 1.
///
/// FNV i stedet for SHA-256, så kernen forbliver uden kryptoafhængighed.
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

/// Deler en kommasepareret liste (`"dk, nordjylland"`) i navne.
pub fn names(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// Id'er for en kommasepareret liste (højst [`MAX_REGIONS`]).
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

/// Deler et kanalnavn `navn@region` i (navn, region). Uden `@` er regionen tom.
pub fn split_channel(name: &str) -> (&str, &str) {
    match name.rsplit_once('@') {
        Some((n, r)) => (n.trim(), r.trim()),
        None => (name.trim(), ""),
    }
}

/// Må en node med `regions` (og `strict`) videresende et flood i `region`?
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
        // Uden region: altid.
        assert!(forwards(&[], false, 0));
        assert!(forwards(&[], true, 0));
        assert!(forwards(&[dk], true, 0));
        // Tom liste: alt, medmindre strict.
        assert!(forwards(&[], false, nj));
        assert!(!forwards(&[], true, nj));
        // Liste: kun egne regioner.
        assert!(forwards(&[dk, nj], false, nj));
        assert!(forwards(&[dk], true, dk));
        assert!(!forwards(&[dk], false, nj));
    }
}
