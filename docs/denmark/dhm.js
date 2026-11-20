// Precise heights from Danmarks Højdemodel (DHM) for the radio path tool,
// when the viewer has their own Dataforsyningen key; without a key the
// simulator keeps its built-in heights (Terrarium / SRTM, ~30 m).
//
// * Terrain (dhm_terraen): the bare ground, 0.4 m model, read here at 2 m
//   or coarser so a profile is about 1000 samples.
// * Surface (dhm_overflade): ground plus trees and buildings.
// * GeoDanmark map layers name what stands on the ground (building,
//   forest, hedge, tree group).
// The key stays in memory unless the user explicitly chooses to remember it.
(function () {
  const WCS = "https://api.dataforsyningen.dk/dhm_wcs_DAF";
  const WMS = "https://api.dataforsyningen.dk/GeoDanmark_60_NOHIST_DAF";
  const STORE = "maarse.dataforsyningen.key";

  let key = "";
  const OPT_IN = STORE + ".remember";
  let remembered = false;
  try {
    if (localStorage.getItem(OPT_IN) === "yes") {
      key = localStorage.getItem(STORE) || "";
      remembered = Boolean(key);
    } else {
      localStorage.removeItem(STORE);
    }
  } catch { /* storage blocked */ }
  function setKey(value, remember = false) {
    key = (value || "").trim();
    remembered = false;
    try {
      if (key && remember) {
        localStorage.setItem(STORE, key);
        localStorage.setItem(OPT_IN, "yes");
        remembered = true;
      } else {
        localStorage.removeItem(STORE);
        localStorage.removeItem(OPT_IN);
      }
    } catch { /* storage blocked */ }
  }

  // WGS84 -> UTM zone 32 (ETRS89, EPSG:25832), the grid DHM is served in.
  function utm32(lat, lon) {
    const a = 6378137, f = 1 / 298.257222101, k0 = 0.9996, lon0 = 9 * Math.PI / 180;
    const e2 = f * (2 - f), ep2 = e2 / (1 - e2);
    const phi = lat * Math.PI / 180, lam = lon * Math.PI / 180;
    const N = a / Math.sqrt(1 - e2 * Math.sin(phi) ** 2);
    const T = Math.tan(phi) ** 2, C = ep2 * Math.cos(phi) ** 2, A = Math.cos(phi) * (lam - lon0);
    const M = a * ((1 - e2 / 4 - 3 * e2 ** 2 / 64 - 5 * e2 ** 3 / 256) * phi - (3 * e2 / 8 + 3 * e2 ** 2 / 32 + 45 * e2 ** 3 / 1024) * Math.sin(2 * phi)
      + (15 * e2 ** 2 / 256 + 45 * e2 ** 3 / 1024) * Math.sin(4 * phi) - (35 * e2 ** 3 / 3072) * Math.sin(6 * phi));
    const x = 500000 + k0 * N * (A + (1 - T + C) * A ** 3 / 6 + (5 - 18 * T + T * T + 72 * C - 58 * ep2) * A ** 5 / 120);
    const y = k0 * (M + N * Math.tan(phi) * (A * A / 2 + (5 - T + 9 * C + 4 * C * C) * A ** 4 / 24 + (61 - 58 * T + T * T + 600 * C - 330 * ep2) * A ** 6 / 720));
    return [x, y];
  }

  // Minimal GeoTIFF reader: one band of 32-bit floats, strips, no
  // compression or deflate (what the DHM service sends).
  async function readTiff(buf) {
    const v = new DataView(buf), le = v.getUint16(0) === 0x4949;
    const u16 = (o) => v.getUint16(o, le), u32 = (o) => v.getUint32(o, le);
    const ifd = u32(4), n = u16(ifd), tags = {};
    for (let i = 0; i < n; i++) {
      const o = ifd + 2 + i * 12, tag = u16(o), type = u16(o + 2), count = u32(o + 4);
      const size = type === 3 ? 2 : 4, at = count * size > 4 ? u32(o + 8) : o + 8;
      tags[tag] = Array.from({ length: count }, (_, k) => type === 3 ? u16(at + k * 2) : u32(at + k * 4));
    }
    const w = tags[256][0], h = tags[257][0], comp = (tags[259] || [1])[0];
    if ((tags[258] || [32])[0] !== 32 || (tags[339] || [3])[0] !== 3 || (tags[317] || [1])[0] !== 1) throw new Error("unsupported TIFF");
    const offs = tags[273], counts = tags[279], parts = [];
    for (let k = 0; k < offs.length; k++) {
      let p = new Uint8Array(buf, offs[k], counts[k]);
      if (comp === 8 || comp === 32946) p = new Uint8Array(await new Response(new Blob([p]).stream().pipeThrough(new DecompressionStream("deflate"))).arrayBuffer());
      else if (comp !== 1) throw new Error("unsupported TIFF compression " + comp);
      parts.push(p);
    }
    const all = new Uint8Array(w * h * 4);
    let o = 0;
    for (const p of parts) { all.set(p.subarray(0, Math.min(p.length, all.length - o)), o); o += p.length; }
    const dv = new DataView(all.buffer), out = new Float32Array(w * h);
    for (let i = 0; i < out.length; i++) out[i] = dv.getFloat32(i * 4, le);
    return { w, h, z: out };
  }

  async function coverage(cov, e0, n0, e1, n1, w, h) {
    const q = new URLSearchParams({ service: "WCS", version: "1.0.0", request: "GetCoverage", coverage: cov, crs: "EPSG:25832",
      bbox: [e0, n0, e1, n1].map((x) => x.toFixed(1)).join(","), width: w, height: h, format: "GTiff", token: key });
    const r = await fetch(`${WCS}?${q}`);
    if (r.status === 401 || r.status === 403) throw new Error("the key was refused");
    if (!r.ok) throw new Error("DHM answered " + r.status);
    const buf = await r.arrayBuffer();
    if (new Uint8Array(buf, 0, 1)[0] === 0x3c) throw new Error("DHM: " + new TextDecoder().decode(buf).replace(/<[^>]+>/g, " ").trim().slice(0, 120));
    return readTiff(buf);
  }

  // Heights along a to b (lat/lon): N+1 samples of terrain and surface.
  // The line is cut into pieces whose boxes are at most 100 x 100 cells.
  async function profile(a, b, N) {
    const [ea, na] = utm32(a.lat, a.lng), [eb, nb] = utm32(b.lat, b.lng);
    const D = Math.hypot(eb - ea, nb - na);
    const res = Math.max(2, D / N);              // metres per cell
    const piece = 90 * res;                      // metres of line per request
    const k = Math.max(1, Math.ceil(D / piece));
    const terrain = new Float32Array(N + 1), surface = new Float32Array(N + 1);
    const jobs = [];
    for (let p = 0; p < k; p++) {
      const i0 = Math.floor(N * p / k), i1 = Math.floor(N * (p + 1) / k);
      const pts = [];
      for (let i = i0; i <= i1; i++) pts.push([i, ea + (eb - ea) * i / N, na + (nb - na) * i / N]);
      const xs = pts.map((q) => q[1]), ys = pts.map((q) => q[2]);
      const e0 = Math.min(...xs) - res * 2, e1 = Math.max(...xs) + res * 2, n0 = Math.min(...ys) - res * 2, n1 = Math.max(...ys) + res * 2;
      const w = Math.max(2, Math.ceil((e1 - e0) / res)), h = Math.max(2, Math.ceil((n1 - n0) / res));
      const read = (g, out) => {
        for (const [i, e, n] of pts) {
          const x = Math.min(g.w - 1, Math.max(0, Math.floor((e - e0) / (e1 - e0) * g.w)));
          const y = Math.min(g.h - 1, Math.max(0, Math.floor((n1 - n) / (n1 - n0) * g.h)));
          const val = g.z[y * g.w + x];
          out[i] = val > -100 && val < 1000 ? val : 0;      // no-data (sea) -> 0
        }
      };
      jobs.push(coverage("dhm_terraen", e0, n0, e1, n1, w, h).then((g) => read(g, terrain)));
      jobs.push(coverage("dhm_overflade", e0, n0, e1, n1, w, h).then((g) => read(g, surface)));
    }
    await Promise.all(jobs);
    return { terrain: Array.from(terrain), surface: Array.from(surface), res };
  }

  // What stands at a point: GeoDanmark layers drawn in a small box around it.
  const KINDS = [["BYGNING", "building"], ["SKOV", "forest"], ["TRAEGRUPPE", "tree group"], ["HEGN", "hedge"], ["HOEJ_BEBYGGELSE", "town (tall)"], ["LAV_BEBYGGELSE", "town (low)"]];
  async function whatIsAt(lat, lon) {
    const [e, n] = utm32(lat, lon), r = 6;
    const found = [];
    await Promise.all(KINDS.map(async ([layer, name]) => {
      const q = new URLSearchParams({ service: "WMS", version: "1.3.0", request: "GetMap", layers: layer, styles: "", crs: "EPSG:25832",
        bbox: [e - r, n - r, e + r, n + r].map((x) => x.toFixed(1)).join(","), width: 6, height: 6, format: "image/png", transparent: "TRUE", token: key });
      try {
        const res = await fetch(`${WMS}?${q}`);
        if (!res.ok) return;
        const bm = await createImageBitmap(await res.blob());
        const c = document.createElement("canvas"); c.width = c.height = 6;
        const g = c.getContext("2d"); g.drawImage(bm, 0, 0);
        const d = g.getImageData(0, 0, 6, 6).data;
        for (let i = 3; i < d.length; i += 4) if (d[i] > 0) { found.push(name); return; }
      } catch { /* layer not available */ }
    }));
    return KINDS.map((k) => k[1]).filter((k) => found.includes(k));
  }

  // A small request to tell the viewer whether the key works.
  async function test() {
    const [e, n] = utm32(55.9787, 9.8288);   // Ejer Bavnehøj
    const g = await coverage("dhm_terraen", e - 10, n - 10, e + 10, n + 10, 10, 10);
    return Math.max(...g.z);
  }

  window.DKDhm = { get key() { return key; }, get remembered() { return remembered; }, setKey, profile, whatIsAt, test };
})();
