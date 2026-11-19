// Demo maarse:// pages for the Denmark simulator: useful-looking sites
// (ferries, weather, emergency info) on hosts in every region. The host
// nearest each town gets the site; it announces it (CAP_PAGES and its name)
// like every other host, so phones find it under Pages.
//
// All times and numbers are examples for the simulator, not real
// timetables or forecasts; each page says so.
(function (root) {
  "use strict";
  const style = "@bg #0f1418\n@fg #e8ecef\n@link #8fd3ff\n@font sans\n";
  const demo = "\n-> {#8b969e:Example data for the simulator, not a real timetable or forecast.} <-\n";
  const foot = (links) => `\n-> {#8b969e:${links.map(([t, p]) => `[${t}](${p})`).join(" · ")}} <-\n${demo}`;
  const table = (head, rows) => `| ${head.join(" | ")} |\n|${head.map(() => "---").join("|")}|\n${rows.map((r) => `| ${r.join(" | ")} |`).join("\n")}\n`;

  const SITES = [
    {
      region: "nordjylland", near: [57.441, 10.537], name: "Nordjylland Harbours",
      pages: {
        index: `@title Nordjylland Harbours\n${style}\n# Nordjylland Harbours\nFerries from Frederikshavn and Hirtshals, and the weather at sea.\n\n::: cards 3\n### [Ferries](ferries)\nGöteborg, Kristiansand, Larvik, Læsø\n+++\n### [Sea weather](weather)\nWind and waves, Skagerrak and Kattegat\n+++\n### [Harbour office](office)\nOpening hours and VHF channels\n:::\n${foot([["Front page", "/"]])}`,
        ferries: `@title Ferries\n${style}\n## Departures today\n${table(["From", "To", "Times", "Crossing"], [["Frederikshavn", "Göteborg", "07:15 · 12:30 · 18:45", "3 h 30"], ["Hirtshals", "Kristiansand", "08:00 · 20:15", "3 h 15"], ["Hirtshals", "Larvik", "11:30", "3 h 45"], ["Frederikshavn", "Læsø", "06:00 · 09:30 · 14:00 · 19:30", "1 h 30"]])}\n>!yellow **Strong wind from 16:00:** the evening departures may be late.\n${foot([["Front page", "/"], ["Sea weather", "weather"]])}`,
        weather: `@title Sea weather\n${style}\n## Sea weather\n${table(["Area", "Wind", "Waves", "Sight"], [["Skagerrak", "SW 12-15 m/s", "2.5 m", "good"], ["Kattegat north", "SW 9-12 m/s", "1.5 m", "good"], ["Læsø Rende", "W 8-10 m/s", "1 m", "fog patches at dawn"]])}\n${foot([["Front page", "/"], ["Ferries", "ferries"]])}`,
        office: `@title Harbour office\n${style}\n## Harbour office\nOpen every day 07:00-19:00.\n\n- VHF channel 12 (Frederikshavn), 14 (Hirtshals)\n- Fuel at pier 3, cash and card\n- Guest berths: ask on VHF before you come in\n${foot([["Front page", "/"]])}`,
      },
    },
    {
      region: "midtjylland", near: [56.157, 10.211], name: "Aarhus Weather",
      pages: {
        index: `@title Aarhus Weather\n${style}\n# Aarhus Weather\n\n${table(["", "Today", "Tomorrow", "Day after"], [["Sky", "showers", "cloudy", "sun"], ["Temp", "9 °C", "11 °C", "13 °C"], ["Wind", "W 8 m/s", "SW 6 m/s", "S 4 m/s"], ["Rain", "4 mm", "1 mm", "0 mm"]])}\n- [Warnings](warnings)\n- [Ferry to Odden](ferry)\n${foot([["Front page", "/"]])}`,
        warnings: `@title Weather warnings\n${style}\n## Warnings for Midtjylland\n>!yellow **Strong wind** along the west coast tonight, gusts up to 20 m/s.\n\n>!blue **Flooding** possible at Randers Fjord at high water, 03:40.\n${foot([["Front page", "/"]])}`,
        ferry: `@title Aarhus-Odden\n${style}\n## Aarhus - Odden (Sjællands Odde)\n${table(["Aarhus", "Odden"], [["06:00", "07:15"], ["09:30", "10:45"], ["13:00", "14:15"], ["17:15", "18:30"], ["21:00", "22:15"]])}\n${foot([["Front page", "/"]])}`,
      },
    },
    {
      region: "syddanmark", near: [55.476, 8.459], name: "Esbjerg & Fanø",
      pages: {
        index: `@title Esbjerg & Fanø\n${style}\n# Esbjerg & Fanø\n- [Fanø ferry](ferry): every 20-40 minutes\n- [Tides](tides): high and low water\n- [Beach and wind](beach)\n${foot([["Front page", "/"]])}`,
        ferry: `@title Fanø ferry\n${style}\n## Esbjerg - Nordby, 12 minutes\nFrom Esbjerg: every 20 min 06:00-19:00, then every 40 min until 00:20.\nFrom Nordby: on the 10s, 30s and 50s in the day.\n\n>!green The ferry runs normally today.\n${foot([["Front page", "/"], ["Tides", "tides"]])}`,
        tides: `@title Tides\n${style}\n## Tides, Esbjerg\n${table(["", "Time", "Height"], [["High", "03:40", "+0.9 m"], ["Low", "09:55", "-0.6 m"], ["High", "16:05", "+1.0 m"], ["Low", "22:20", "-0.5 m"]])}\n${foot([["Front page", "/"]])}`,
        beach: `@title Beach and wind\n${style}\n## Fanø beach\nWind W 9 m/s, water 11 °C, flag yellow (take care).\n${foot([["Front page", "/"]])}`,
      },
    },
    {
      region: "sjaelland", near: [55.681, 11.089], name: "Kalundborg Port",
      pages: {
        index: `@title Kalundborg Port\n${style}\n# Kalundborg Port\n- [Ferry to Samsø](samso)\n- [Roads and trains](roads)\n- [Pharmacy and doctor on call](health)\n${foot([["Front page", "/"]])}`,
        samso: `@title Kalundborg-Samsø\n${style}\n## Kalundborg - Ballen (Samsø), 1 h 50\n${table(["Kalundborg", "Ballen"], [["07:30", "09:20"], ["12:15", "14:05"], ["17:45", "19:35"]])}\n${foot([["Front page", "/"]])}`,
        roads: `@title Roads and trains\n${style}\n## Roads and trains\n- Route 23: road works at Jyderup, one lane.\n- Trains to Copenhagen every 30 minutes.\n${foot([["Front page", "/"]])}`,
        health: `@title Health\n${style}\n## Doctor on call\nPhone 1813. Pharmacy on Kordilgade open until 20:00.\n${foot([["Front page", "/"]])}`,
      },
    },
    {
      region: "hovedstaden", near: [55.676, 12.568], name: "Copenhagen Emergency Info",
      pages: {
        index: `@title Copenhagen Emergency Info\n${style}\n# Copenhagen Emergency Info\nWhat to do when power, phones or internet are down.\n\n::: cards 3\n### [Water points](water)\nWhere to get drinking water\n+++\n### [Shelters](shelters)\nWarm rooms and shelters\n+++\n### [Ferry to Bornholm](bornholm)\nKøge - Rønne\n:::\n>!blue Listen to DR P4 on FM for official messages.\n${foot([["Front page", "/"]])}`,
        water: `@title Water points\n${style}\n## Water points\n- Rådhuspladsen, by the fountain\n- Nørrebro Park, north gate\n- Amager Strandpark, car park\n\nBring your own containers. 20 litres per person.\n${foot([["Front page", "/"]])}`,
        shelters: `@title Shelters\n${style}\n## Warm rooms and shelters\n${table(["Place", "Open", "Beds"], [["Øksnehallen", "24 h", "400"], ["Bella Center hall C", "24 h", "1200"], ["DGI-byen", "08-22", "-"]])}\n${foot([["Front page", "/"]])}`,
        bornholm: `@title Køge-Rønne\n${style}\n## Køge - Rønne, 5 h 30\n${table(["Køge", "Rønne"], [["00:30", "06:00"], ["08:30", "14:00"], ["16:30", "22:00"]])}\n${foot([["Front page", "/"]])}`,
      },
    },
    {
      region: "bornholm", near: [55.101, 14.700], name: "Bornholm Ferries",
      pages: {
        index: `@title Bornholm Ferries\n${style}\n# Bornholm Ferries\n${table(["Route", "Next", "Time"], [["Rønne - Ystad", "10:30", "1 h 20"], ["Rønne - Køge", "16:30", "5 h 30"], ["Rønne - Sassnitz", "14:00", "3 h 30"]])}\n- [All departures](all)\n- [Weather on the Baltic](weather)\n${foot([["Front page", "/"]])}`,
        all: `@title All departures\n${style}\n## Rønne - Ystad\n06:00 · 08:30 · 10:30 · 13:00 · 15:30 · 18:00 · 20:30\n\n## Ystad - Rønne\n07:30 · 10:00 · 12:00 · 14:30 · 17:00 · 19:30 · 22:00\n${foot([["Front page", "/"]])}`,
        weather: `@title Baltic weather\n${style}\n## Baltic, Bornholm\nWind SE 7 m/s, waves 1 m, rain in the afternoon.\n${foot([["Front page", "/"]])}`,
      },
    },
  ];

  /** Puts the demo sites on the host nearest each town (one site per host). */
  function addDemoSites(sc) {
    const km = (a, b) => { const dy = (a[0] - b[0]) * 111.32, dx = (a[1] - b[1]) * 111.32 * Math.cos(a[0] * Math.PI / 180); return Math.hypot(dx, dy); };
    const used = new Set();
    for (const s of SITES) {
      let best = -1, bd = 1e9;
      sc.nodes.forEach((n, i) => {
        if (n.kind !== "host" || used.has(i)) return;
        const d = km([n.lat, n.lon], s.near);
        if (d < bd) { bd = d; best = i; }
      });
      if (best < 0 || bd > 80) continue;
      used.add(best);
      sc.nodes[best].site = { name: s.name, pages: Object.assign({}, s.pages) };
    }
    return sc;
  }

  root.DK_DEMO_SITES = { SITES, addDemoSites };
})(typeof window !== "undefined" ? window : globalThis);
