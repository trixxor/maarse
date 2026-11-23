# Offline map of Denmark

Status: host side built and tested in the working copy (branch `map-dk-offline`); the web client draws it (see below); not released, the `map-dk`
release does not exist yet, and neither the web client nor the Android app draws the map from it yet.

The map is one free vector tile archive (Protomaps basemap, OpenStreetMap data) that the host keeps on disk
and serves to the web client and the Android app. Once it is installed nothing needs the internet to draw
the map. Server mode (the dedicated VPS `maarse-server`) does not serve the map.

## Online by default, offline file optional (web client, working copy)

The web client's Map tab draws the real map without any download. The source is chosen in this order:

1. **The installed map file** (`status.installed`): drawn from `/map/denmark.pmtiles`; no internet needed, nothing leaves this computer.
2. **OpenFreeMap online** (`https://tiles.openfreemap.org/styles/positron` in the light theme, `.../styles/dark` in the dark theme): free, no key, viewing is unrestricted. Attribution shown on the map: "OpenFreeMap © OpenMapTiles Data from OpenStreetMap". Only used while no file is installed and the person has not turned it off.
3. **The offline grid** (no tiles) when neither works: no file and no internet, or the online map is turned off, or the map libraries did not load. A card says why and offers the download.

The attribution always matches the source in use. The node overlay is the same on both sources.

**Privacy.** With the online map the OpenFreeMap server sees the IP address of the computer and the area that is viewed (through the tiles it asks for). It is the only request the web client makes to another server for the map; the style, fonts, sprites and libraries of the online map come from `tiles.openfreemap.org` too, those of the file map from the host. The map screen has a button "Use without internet" (the offline download panel) with a switch "Use online map" (default on, kept in the browser's local storage as `maarse:map-online`). Installing a map file removes the need for the online map entirely. There is no setting for the online map address: the standard default is built in.

The download panel lives in the Map tab (button "Use without internet", "Offline map" once a file is installed): installed level and size, Overview and Detailed with the real sizes (from `POST /api/map/check`), progress while downloading (polls `/api/map/status` every second), Cancel (resumes later), Delete, plain-language errors and the licences. The service worker caches the libraries, style, fonts and sprites under `/map/` in its own cache (`maarse-map-v1`) and never `denmark.pmtiles` (range responses) or `/api/`.

Verified in headless Edge against a test host (light and dark; 1920x1080, 1280x800, 800x900, 390x844; no horizontal scroll): online map, installed file with the internet blocked (no external request), download, cancel, resume, delete, fallback grid without internet. The Android app does not do this yet.

## How it works

1. **We publish our own extracts.** The manual workflow `.github/workflows/map.yml` cuts two extracts out of a
   Protomaps daily build and uploads them, with `map-manifest.json`, to the rolling pre-release `map-dk` of this
   repository. Nothing links to Protomaps' servers (they ask not to be hotlinked).
2. **The host downloads one level** from the manifest (`<data>/map/`): resumed with HTTP Range after a restart
   or a network failure, checked against the SHA-256 in the manifest, installed with an atomic rename as
   `<data>/map/denmark.pmtiles` and described by `installed.json`. A wrong size or hash deletes the part file
   and the old map stays. Only one level is installed at a time; installing the other replaces it.
3. **The host serves it** at `/map/denmark.pmtiles` with `Range` support, and the client libraries at `/map/*`.
   The clients draw it with MapLibre GL and the PMTiles protocol.

| Level | Zoom | Size (measured) | Use |
|---|---|---|---|
| `overview` (default) | 0-10 | 22.5 MB | whole country, towns, main roads, coast |
| `detailed` (optional) | 0-13 | about 185 MB | streets and buildings in towns |

Downloading needs `curl` on the host (Windows 10+ and Linux have it). The Android app has none: it downloads
the file itself (see "API").

## Measured sizes

Protomaps build 20261123 (basemap version 4.15.2), `pmtiles` 1.31.2, measured 2026-11-23 with `pmtiles extract
... --dry-run` (archive size) and, for the overview, a real extract:

| maxzoom | bbox 7.9,54.5,15.3,57.8 | Denmark region (`tools/map/denmark-region.geojson`) |
|---|---|---|
| 10 | 30 MB (547 tiles) | **22 MB**, real file 22,495,666 bytes |
| 12 | 126 MB | not measured |
| 13 | 250 MB | **185 MB** |
| 14 | 470 MB | not measured |

Each extra zoom level roughly doubles the size, so z14 (470 MB) was left out.

**Region instead of a bounding box.** The bbox needs lon up to 15.3 for Bornholm, which drags in southern Sweden
and a lot of sea. `tools/map/denmark-region.geojson` is a two-part polygon (the mainland and islands with their
seas up to lon 12.8, and a box around Bornholm). It is hand-made and crude, and still saves about 26 percent
(z13: 250 to 185 MB, z10: 30 to 22 MB). Consequence: at zoom levels above the coarsest, Skane and the German
side of the border are mostly empty. A true Denmark outline would save more but needs a boundary file with its own
licence; the crude polygon is good enough and has no extra licence. Replace the file to change the cut.

## Settings

`settings.txt` (and the web client's Settings tab, which edits the same file):

```
--map-url https://example.org/maps/map-manifest.json
```

The address of the manifest. Default: `https://github.com/trixxor/maarse/releases/download/map-dk/map-manifest.json`.
The files are expected next to the manifest, under the names it lists. Only `http://` and `https://` are
accepted; file names in a manifest must be `[a-z0-9._-]+.pmtiles`, so a manifest can never write outside
`<data>/map/`. The settings screens (web and Android) have no field for this yet: it is an advanced option, the
level choice lives in the map screen.

## Access and security choices

* `GET`/`HEAD /map/*` need **no access key**, also with `--require-key`. The map is public data, the web page
  itself is already served without a key, and a map library cannot easily attach a key to every tile request.
  Nothing in `/map/*` depends on the user: only fixed file names are served, no part of a URL is ever used as a
  path, and POST is refused with 405. Server mode does not route `/map/` at all.
* `/api/map/*` follows the same rule as every other `/api/*` call: local page or access key.
* `/map/` replies carry `Cross-Origin-Resource-Policy: same-origin` and no CORS headers: only the page the host
  itself serves (or a native app) reads them.
* Write timeout for `/map/` is 60 s (the rest of the web server uses 5 s), so a slow phone can finish a range.
* Caching: `denmark.pmtiles` has an `ETag` (size and mtime) and `Cache-Control: no-cache`, so a client asks
  again and gets 304 or the new file after an update. Library files, fonts and sprites have an `ETag` (host
  version and length) and `max-age=3600`.
* The manifest is not signed (the sensor catalogue is). It comes over HTTPS from the project's release, and the file
  is checked against the hash in it, so the trust is the same as the GitHub release itself. The data is only parsed by
  the map library in the browser, never run.

## API (for the clients)

Mutating calls need the access key like other `/api/*` calls. All answers are JSON.

`GET /api/map/status`

```json
{
  "enabled": true,
  "attribution": "© OpenStreetMap contributors · Protomaps",
  "manifest_url": "https://github.com/trixxor/maarse/releases/download/map-dk/map-manifest.json",
  "can_download": true,
  "tiles_url": "/map/denmark.pmtiles",
  "installed": {"level":"overview","name":"denmark-overview.pmtiles","bytes":22495666,"sha256":"…","build":"20261123","minzoom":0,"maxzoom":10,"bbox":[7.9,54.5,15.3,57.8],"installed_at":1795000000},
  "available": [{"level":"overview","name":"…","bytes":22495666,"minzoom":0,"maxzoom":10,"bbox":[…],"part_path":"…","part_bytes":0}],
  "build": "20261123", "osm_data": "2026-11-23T04:00:00Z", "checked_at": 1795000000,
  "download": {"level":"detailed","phase":"downloading","received":1048576,"total":194000000},
  "error": "",
  "dir": "<data>/map"
}
```

`installed`, `tiles_url` and `download` are `null` when there is none. `available` is empty until
`check` (or a download) has fetched the manifest. `phase` is `manifest`, `downloading` or `verifying`
(`received` then counts hashed bytes). `error` keeps the last failure until the next attempt.

* `POST /api/map/check`: fetch the manifest now, answers the status (502 `{"error":…}` on failure).
* `POST /api/map/download?level=overview|detailed`: start (or resume) in the background and answer the status; poll
  `status`. 409 if one is already running, 400 for another level.
* `POST /api/map/cancel`: stop the running download (the part file is kept for resume).
* `POST /api/map/delete`: remove the installed map and part files (409 while downloading).
* `POST /api/map/manifest`: body = manifest JSON; for devices without `curl` that fetched it themselves.

**Android (no curl).** Fetch the manifest yourself, `POST /api/map/manifest`, download the chosen file to
`part_path` (resume from `part_bytes` with a Range request), then `POST /api/map/download?level=…`: a complete part
file is only verified and installed, nothing is fetched.

**Drawing.** In the page: load `/map/maplibre-gl.css`, `/map/maplibre-gl.js`, `/map/pmtiles.js`; then

```js
maplibregl.addProtocol('pmtiles', new pmtiles.Protocol().tile);
const style = await (await fetch('/map/style.json?flavor=' + (dark ? 'dark' : 'light'))).json();
new maplibregl.Map({container, style, center: [10.2, 56.2], zoom: 6.5});
```

`/map/style.json` is built from the `Host` header, so tiles, glyphs (`/map/fonts/{fontstack}/{range}.pbf`) and
sprites (`/map/sprite/light|dark[@2x].json|png`) come from the same host the page was loaded from. A native
MapLibre client uses the same style URL with the host's address. The source has `maxzoom` 14 so MapLibre keeps
drawing (over-zoomed) beyond the installed level. Show the attribution (it is also in the style).
This was checked in Chrome against a real overview file: no errors, labels in Danish, light flavour.

## What is vendored (`maarse-client/map/`, compiled into the host)

| Part | Version | Licence |
|---|---|---|
| MapLibre GL JS (`maplibre-gl.js`, `.css`) | 5.24.1 (UMD build; 6.x ships ES modules only) | BSD-3-Clause |
| PMTiles JS (`pmtiles.js`) | 4.5.0 | BSD-3-Clause (the package says BSD-3-Clause; there is no MIT licence file) |
| Style (`style-light.json`, `style-dark.json`) | generated from `@protomaps/basemaps` 5.7.2, `layers()` with `lang: "da"` | BSD-3-Clause; data ODbL |
| Fonts: Noto Sans Regular, Medium, Italic, ranges 0-255, 256-511, 8192-8447 | `protomaps/basemaps-assets` commit 028c18f | SIL OFL 1.1 (Noto Project Authors) |
| Sprites (light, dark, 1x and 2x) | same commit, v4 | MIT (derived from tangrams/icons, Mapzen) |

Licence texts are kept next to the files (`LICENSE-*`). The Devanagari font the style names for Indic scripts is
not included (a missing glyph range only means those labels are not drawn). The map data is ODbL: attribution
"© OpenStreetMap contributors · Protomaps" must stay visible on every map. Binary growth: about 2.3 MB
(MapLibre 1.06 MB, its CSS 70 KB, PMTiles 20 KB, two styles 130 KB, fonts 0.8 MB, sprites 0.1 MB).

To update a library: `npm pack` the exact version into an empty folder, copy the same files, run the style
generator (`layers('protomaps', namedFlavor(flavor), {lang:'da'})`, with `{{ORIGIN}}` placeholders for glyphs,
sprite and the `pmtiles://` source URL), and update the table above.

## Publishing a map (what you run)

1. GitHub > Actions > **Offline map of Denmark** > Run workflow (leave *build* empty for the newest Protomaps
   build, or give a date such as `20261123`). It takes a few minutes; the detailed extract is read from Protomaps once.
2. It creates or updates the pre-release `map-dk` with `denmark-overview.pmtiles`, `denmark-detailed.pmtiles` and
   `map-manifest.json` (name, bytes, sha256, bbox, minzoom, maxzoom, build date, OSM data date, attribution). The
   manifest is uploaded last. A host that is halfway through a download of the old file gets a checksum error,
   deletes it and downloads again.
3. The workflow was written but never run. Its first run should be watched; the `pmtiles` download is pinned to
   v1.31.2 with the SHA-256 that GitHub reports for the Linux archive (Protomaps publishes no checksum file).

Run it a few times a year; roads and towns change slowly.
