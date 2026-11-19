// Rule and radio profiles (Build > Rules and radio).
//
// "Standard (as built)" comes from the engine: the host's rules from
// maarse-hostrules (Rules::STANDARD) and the firmware's radio default
// (RadioConfig::EU_DEFAULT), so it follows the code. A profile can be copied,
// changed and saved under a name (in this browser); applying it writes the
// values into the scenario and rebuilds the run.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const KEY = "dk-profiles";
  const STD = "Standard (as built)";

  // Field list: [key, label, unit, scale]. scale turns the stored value into
  // the shown one (ms -> s or min).
  const RADIO = [
    ["sf", "Spreading factor", "", 1],
    ["freq_mhz", "Frequency", "MHz", 1],
    ["bw_hz", "Bandwidth", "kHz", 1 / 1000],
    ["tx_dbm", "TX power", "dBm", 1],
    ["duty_permille", "Duty cycle", "‰", 1],
  ];
  const RULES = [
    ["deposit_after_ms", "To the post office after no ack for", "s", 1 / 1000],
    ["mailbox_per_recipient", "Mailbox: messages per recipient", "", 1],
    ["mailbox_ttl_ms", "Mailbox keeps mail for", "days", 1 / 86400000],
    ["mail_present_ms", "Recipient counts as present for", "min", 1 / 60000],
    ["mail_fetch_every_ms", "Ask the replicas at most every", "min", 1 / 60000],
    ["mail_retry_first_ms", "First delivery retry after", "min", 1 / 60000],
    ["mail_retry_steps", "Retries before waiting for the recipient", "", 1],
    ["dht_every_ms", "Backbone DHT upkeep every", "s", 1 / 1000],
    ["dht_lookup_every_ms", "Same address looked up at most every", "s", 1 / 1000],
    ["bordercast_repeat_ms", "Bordercast: second request goes out within", "s", 1 / 1000],
    ["key_answer_every_ms", "Key requests answered at most every", "s", 1 / 1000],
    ["max_file", "Largest file over the internet", "KB", 1 / 1024],
    ["filebox_on", "File mailbox on", "", "bool"],
    ["pages_burst_lora", "Page requests per burst over LoRa", "", 1],
    ["pages_burst_net", "Page requests per burst over the internet", "", 1],
    ["tun_lora", "IP tunnel may use LoRa", "", "bool"],
  ];

  let standard = null; // { radio, rules }
  const load = () => { try { return JSON.parse(localStorage.getItem(KEY) || "{}"); } catch (e) { return {}; } };
  const save = (all) => { try { localStorage.setItem(KEY, JSON.stringify(all)); return true; } catch (e) { return false; } };

  function getStandard() {
    if (standard) return standard;
    const sim = window.DKSim && window.DKSim.sim;
    if (!sim) return null;
    const r = sim.call({ cmd: "rules" });
    standard = { radio: r.radio_standard, rules: r.standard };
    return standard;
  }

  function profile(name) {
    const std = getStandard();
    if (!std) return null;
    if (name === STD) return { radio: Object.assign({}, std.radio), rules: Object.assign({}, std.rules) };
    const p = load()[name];
    return p ? { radio: Object.assign({}, std.radio, p.radio), rules: Object.assign({}, std.rules, p.rules) } : null;
  }

  function fieldHtml(group, [key, label, unit, scale], value, std) {
    const id = `pr-${group}-${key}`;
    const changed = value !== std;
    if (scale === "bool") {
      return `<label class="pr-row${changed ? " pr-changed" : ""}"><span>${label}</span><input type="checkbox" id="${id}" ${value ? "checked" : ""}></label>`;
    }
    const shown = +(value * scale).toFixed(4);
    const was = changed ? ` title="Standard: ${+(std * scale).toFixed(4)} ${unit}"` : "";
    return `<label class="pr-row${changed ? " pr-changed" : ""}"${was}><span>${label}</span><span><input type="number" id="${id}" value="${shown}" step="any" style="width:${key === "freq_mhz" ? 100 : 80}px"> ${unit}</span></label>`;
  }

  function readFields(group, list, into) {
    for (const [key, , , scale] of list) {
      const el = $(`pr-${group}-${key}`);
      if (!el) continue;
      if (scale === "bool") into[key] = el.checked;
      else {
        const v = +el.value / scale;
        // Integers stay integers (the engine reads them as u32/u64/usize).
        into[key] = key === "freq_mhz" ? v : Math.round(v);
      }
    }
    return into;
  }

  function render() {
    const box = $("pr-body");
    if (!box) return;
    const std = getStandard();
    if (!std) { box.innerHTML = `<p class="hint">Loading the engine…</p>`; setTimeout(render, 500); return; }
    const all = load();
    const names = [STD, ...Object.keys(all).sort()];
    const sel = box.dataset.sel && names.includes(box.dataset.sel) ? box.dataset.sel : STD;
    const p = profile(sel);
    box.innerHTML = `
      <p class="hint">“${STD}” is taken from the code: the host's rules (maarse-hostrules) and the firmware's radio default. Change values, save them as your own profile, and apply it to this run. Changed values are marked; hover to see the standard. To change the radio in the middle of a run, use Break › Radio settings.</p>
      <div class="row"><select id="pr-sel" aria-label="Profile">${names.map((n) => `<option ${n === sel ? "selected" : ""}>${n.replace(/</g, "&lt;")}</option>`).join("")}</select>
        <button id="pr-apply" class="primary">Apply to this run</button></div>
      <h4>Radio (every node)</h4>
      ${RADIO.map((f) => fieldHtml("radio", f, p.radio[f[0]], std.radio[f[0]])).join("")}
      <h4>Host rules</h4>
      ${RULES.map((f) => fieldHtml("rules", f, p.rules[f[0]], std.rules[f[0]])).join("")}
      <div class="row"><input id="pr-name" placeholder="profile name" size="16" aria-label="Profile name" value="${sel === STD ? "" : sel.replace(/"/g, "&quot;")}">
        <button id="pr-save">Save as</button>${sel === STD ? "" : `<button id="pr-del">Delete</button>`}<button id="pr-reset">Reset to standard</button></div>
      <p class="hint" id="pr-note"></p>`;
    $("pr-sel").onchange = (e) => { box.dataset.sel = e.target.value; render(); };
    $("pr-save").onclick = () => {
      const name = $("pr-name").value.trim();
      if (!name || name === STD) { $("pr-note").textContent = "Give the profile a name of its own."; return; }
      const a = load();
      a[name] = { radio: readFields("radio", RADIO, {}), rules: readFields("rules", RULES, {}) };
      $("pr-note").textContent = save(a) ? `Saved “${name}”.` : "This browser does not allow saving (private window?).";
      box.dataset.sel = name; render();
    };
    if ($("pr-del")) $("pr-del").onclick = () => { const a = load(); delete a[sel]; save(a); box.dataset.sel = STD; render(); };
    $("pr-reset").onclick = () => { box.dataset.sel = STD; render(); };
    $("pr-apply").onclick = () => {
      const sc = window.DKSim.scenario;
      const radio = readFields("radio", RADIO, {});
      const rules = readFields("rules", RULES, Object.assign({}, std.rules));
      sc.sf = radio.sf; sc.freq_mhz = radio.freq_mhz; sc.bw_hz = radio.bw_hz; sc.tx_dbm = radio.tx_dbm; sc.duty_permille = radio.duty_permille;
      // Nodes that override the default spreading factor follow the profile too.
      for (const n of sc.nodes) if (n.sf) n.sf = radio.sf;
      sc.rules = rules;
      sc.profile = sel;
      window.DKSim.edited();
      $("pr-note").textContent = `Applied “${sel}” to this run (the run starts again from 0).`;
    };
  }

  window.DKProfiles = { render, STD };
  document.addEventListener("DOMContentLoaded", render);
  if (document.readyState !== "loading") setTimeout(render, 0);
})();
