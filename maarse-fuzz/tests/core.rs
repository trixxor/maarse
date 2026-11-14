//! Fuzz-mål for maarse-core: pakker, companion-rammer og beskedindhold.
//! Kør længere med fx `MAARSE_FUZZ_ITERS=2000000 cargo test -p maarse-fuzz --release`.

use maarse_core::companion::{Command, Event, FrameDecoder, MAX_FRAME, RadioConfig, Text};
use maarse_core::content::{self, FileOffer};
use maarse_core::packet::{self, Header, Packet, PacketType};
use maarse_core::{Address, MAX_PACKET, Path};
use maarse_fuzz::{Rng, run};

fn text(b: &[u8]) -> Text {
    Text::from_slice(b).unwrap()
}

#[test]
fn fuzz_packet_decode() {
    let mut corpus = Vec::new();
    for t in 0u8..16 {
        let Ok(pt) = PacketType::try_from(t) else { continue };
        let h = Header::new(pt, Address([t; 16]));
        corpus.push(packet::encode(&h, b"hej").unwrap().to_vec());
        let ids: [u16; 3] = [1, 2, 0xffff];
        let h = h.with_path(Path::from_slice(&ids).unwrap());
        corpus.push(packet::encode(&h, &[0xaa; 40]).unwrap().to_vec());
    }
    run("packet", &corpus, MAX_PACKET + 8, |b| {
        if let Ok(p) = Packet::decode(b) {
            // Dekodet => kan kodes igen og giver samme pakke.
            let again = packet::encode(&p.header, p.payload).expect("genkodning");
            assert_eq!(Packet::decode(&again).expect("gendekodning"), p);
        }
    });
}

#[test]
fn fuzz_companion_command() {
    let corpus: Vec<Vec<u8>> = [
        Command::GetInfo,
        Command::SendDirect { req: 7, dest: Address([3; 16]), text: text(b"hej med dig") },
        Command::JoinChannel { name: text(b"#aarhus") },
        Command::AnnounceNow,
        Command::FileBegin {
            req: 1,
            dest: Address([4; 16]),
            size: 1000,
            mime: text(b"image/png"),
            name: text(b"a.png"),
        },
        Command::FileData { req: 1, offset: 0, data: text(&[9; 200]) },
    ]
    .iter()
    .map(|c| c.encode()[2..].to_vec())
    .collect();
    run("companion-command", &corpus, MAX_FRAME + 8, |b| {
        // Kun rammer, der kan komme gennem FrameDecoder (<= MAX_FRAME), skal
        // kunne genkodes uden tab.
        if let (true, Ok(c)) = (b.len() <= MAX_FRAME, Command::decode(b)) {
            let f = c.encode();
            assert_eq!(Command::decode(&f[2..]).expect("gendekodning"), c);
        }
    });
}

#[test]
fn fuzz_companion_event() {
    // Ingen kendt korpus her ud over typebytes; tilfældige rammer med
    // tvungen gyldig typebyte giver god dækning.
    let corpus: Vec<Vec<u8>> = (0x80u8..=0xa0)
        .chain(0x00..=0x20)
        .map(|t| {
            let mut v = vec![t];
            v.extend_from_slice(&[0x10; 64]);
            v
        })
        .collect();
    run("companion-event", &corpus, MAX_FRAME + 8, |b| {
        if let (true, Ok(e)) = (b.len() <= MAX_FRAME, Event::decode(b)) {
            let f = e.encode();
            assert_eq!(Event::decode(&f[2..]).expect("gendekodning"), e);
        }
        let _ = RadioConfig::from_bytes(b);
    });
}

#[test]
fn fuzz_frame_decoder_stream() {
    // Strømmen klippes i tilfældige stykker; dekoderen må aldrig gå i panik,
    // hænge eller returnere en ramme over MAX_FRAME.
    let good = Command::JoinChannel { name: text(b"#kbh") }.encode().to_vec();
    let corpus = vec![good.clone(), [good.clone(), good.clone()].concat()];
    let mut rng = Rng::new(1);
    run("frame-decoder", &corpus, 2 * MAX_FRAME + 64, |b| {
        let mut d = FrameDecoder::new();
        let mut rest = b;
        let mut frames = 0usize;
        while !rest.is_empty() {
            let n = 1 + rng.below(rest.len());
            d.push(&rest[..n]);
            rest = &rest[n..];
            while let Some(f) = d.next_frame() {
                assert!(!f.is_empty() && f.len() <= MAX_FRAME);
                frames += 1;
                assert!(frames <= b.len(), "uendelig løkke i next_frame");
            }
        }
    });
    // En gyldig ramme efter skrald skal stadig findes.
    let mut d = FrameDecoder::new();
    d.push(&[0xff; 7]);
    d.push(&good);
    let mut found = false;
    while let Some(f) = d.next_frame() {
        found |= f[..] == good[2..];
    }
    assert!(found);
}

#[test]
fn fuzz_content_parse() {
    let offer = FileOffer {
        file_id: 42,
        size: 1234,
        chunks: 6,
        chunk_size: 200,
        key: [1; 32],
        hash: [2; 16],
        mime: heapless::Vec::from_slice(b"text/plain").unwrap(),
        name: heapless::Vec::from_slice(b"noter.txt").unwrap(),
    };
    let corpus = vec![
        content::text(b"hej").to_vec(),
        content::control(content::CONTROL, 9, &[1, 2, 3]).to_vec(),
        content::control(content::CONTROL_REPLY, 9, &[4]).to_vec(),
        offer.encode().to_vec(),
    ];
    run("content", &corpus, 255, |b| {
        let _ = content::parse(b);
    });
}

/// Router::on_receive med en strøm af pakker fra luften. Inputformat:
/// gentagne `[længde][snr][pakke]`. Routeren beholder tilstand mellem
/// pakkerne, og alt, den selv sender, skal kunne afkodes igen.
#[test]
fn fuzz_router_on_receive() {
    use maarse_core::{Action, Role, Router};
    let me = Address([0x11; 16]);
    let chan = Address([0x22; 16]);
    let my_id = Router::<16, 64>::new(me, Role::Repeater).id;
    let mut pkts = Vec::new();
    for t in 0u8..16 {
        let Ok(pt) = PacketType::try_from(t) else { continue };
        for dest in [me, chan, Address([0x33; 16])] {
            let h = Header::new(pt, dest);
            pkts.push(packet::encode(&h, &[t; 30]).unwrap().to_vec());
            let h = h.with_path(Path::from_slice(&[7, my_id, 9]).unwrap());
            pkts.push(packet::encode(&h, &[t; 60]).unwrap().to_vec());
        }
    }
    let frame = |p: &[u8]| [&[p.len() as u8, 5][..], p].concat();
    let mut corpus: Vec<Vec<u8>> = pkts.iter().map(|p| frame(p)).collect();
    corpus.push(pkts.iter().take(6).flat_map(|p| frame(p)).collect());
    for role in [Role::Repeater, Role::Client, Role::Relay, Role::Anchor] {
        let mut now = 0u64;
        run(&format!("router-{role:?}"), &corpus, 4 * MAX_PACKET, |b| {
            let mut r = Router::<16, 64>::new(me, role);
            let _ = r.subscribe(chan);
            let mut rest = b;
            while rest.len() >= 2 {
                let n = (rest[0] as usize).min(rest.len() - 2);
                let snr = rest[1] as i8;
                let pkt = &rest[2..2 + n];
                rest = &rest[2 + n..];
                now += 1 + snr.unsigned_abs() as u64 * 100;
                let mut acts = r.on_receive(pkt, snr, now);
                acts.extend(r.tick(now + 5_000));
                for a in acts {
                    if let Action::Transmit { packet, .. } = a {
                        Packet::decode(&packet).expect("routeren sendte en ugyldig pakke");
                    }
                }
            }
        });
    }
}

/// RadioConfig fra companion (SetRadio) og flash: kun gyldige indstillinger
/// bruges til sendetid; det må aldrig gå i panik.
#[test]
fn fuzz_radio_config() {
    let corpus = vec![RadioConfig::EU_DEFAULT.to_bytes().to_vec()];
    run("radio-config", &corpus, 20, |b| {
        if let Some(c) = RadioConfig::from_bytes(b) {
            if c.validate().is_ok() {
                for len in [0, 1, MAX_PACKET] {
                    let _ = c.airtime().airtime_ms(len);
                }
            }
        }
    });
}
