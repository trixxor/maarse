// meshcore-host: run one or more virtual MeshCore nodes on a PC, using a
// Heltec (or any board) running the official `kiss_modem` firmware as the LoRa gateway.
//
//   meshcore-host --port COM9 | --tcp 192.168.20.148:5055 --node PC-Alice [--node PC-Bob] [--data .\data] [--verbose]

#include <Arduino.h>
#include "portable.h"
#include <chrono>
#include <algorithm>
#include <filesystem>
#include <map>
#include <fstream>
#include <iostream>
#include <mutex>
#include <queue>
#include <random>
#include <string>
#include <thread>
#include <vector>
#include <memory>
#include "KissHub.h"
#include "VirtualRadio.h"
#include "VirtualNode.h"

// ---- Arduino shim implementations
static const auto t0 = std::chrono::steady_clock::now();
unsigned long millis() {
  return (unsigned long)std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - t0).count();
}
void delay(unsigned long ms) { portable::sleepMs(ms); }
static std::mt19937 g_rng{std::random_device{}()};
long random(long min_v, long max_v) { return max_v <= min_v ? min_v : min_v + (long)(g_rng() % (unsigned long)(max_v - min_v)); }
long random(long max_v) { return random(0, max_v); }
void randomSeed(unsigned long seed) { g_rng.seed(seed); }
ConsoleSerial Serial;

// ---- console input on a background thread
static std::mutex g_mx;
static std::queue<std::string> g_lines;

static void help() {
  printf(
    "Gateway commands:\n"
    "   nodes                  list virtual nodes\n"
    "   use <name|#>           select active node\n"
    "   stats                  modem RX/TX counters\n"
    "   quit\n"
    "Node commands (active node):\n"
    "   advert | advert flood  send self advert (zero hop / flood)\n"
    "   list {n}               contacts\n"
    "   to <name prefix>       select recipient\n"
    "   send <text>            direct message to recipient\n"
    "   public <text>          message on Public channel\n"
    "   card | import <card>   share/import meshcore:// biz card\n"
    "   reset path\n"
    "   set {name|lat|lon|af} <value>\n"
    "   scan {sek}             aktiv scanning efter repeatere (default 10 s)\n"
    "   heard                  passiv liste over noder hørt via adverts\n"
    "   whoami\n");
}

static bool personAddress(const std::string& address) {
  return address.size() == 32 && std::all_of(address.begin(), address.end(), [](char ch) {
    return (ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f');
  });
}

static bool publishPublicKey(const std::filesystem::path& folder, const VirtualNode& node) {
  const auto target = folder / "public-key.txt";
  const auto temporary = folder / ("public-key.new." + std::to_string(portable::processId()));
  constexpr char hexDigits[] = "0123456789abcdef";
  std::ofstream output(temporary, std::ios::binary);
  for (size_t index = 0; index < PUB_KEY_SIZE; ++index) {
    const auto byte = node.publicKey()[index];
    output.put(hexDigits[byte >> 4]);
    output.put(hexDigits[byte & 15]);
  }
  output.close();
  std::error_code renameError;
  if (output) {
    std::filesystem::rename(temporary, target, renameError);
    if (!renameError) return true;
  }
  std::error_code ignored;
  std::filesystem::remove(temporary, ignored);
  return false;
}

static void printProvisioned(const std::string& person, const VirtualNode& node) {
  printf("Provisioned %s ", person.c_str());
  for (size_t index = 0; index < PUB_KEY_SIZE; ++index) printf("%02x", node.publicKey()[index]);
  puts("");
}

// meshcore-host --selftest: the crypto a repeater login relies on, checked on this very build.
static int selfTest() {
  SystemRNG rng;
  mesh::LocalIdentity us(&rng), repeater(&rng);
  uint8_t ours[PUB_KEY_SIZE], theirs[PUB_KEY_SIZE];
  us.calcSharedSecret(ours, repeater.pub_key);
  repeater.calcSharedSecret(theirs, us.pub_key);
  const bool secret = memcmp(ours, theirs, PUB_KEY_SIZE) == 0;
  uint8_t plain[24] = {1, 2, 3, 4};
  memcpy(&plain[4], "sample-password", 15);   // a login: 4-byte time and up to 15 characters
  uint8_t sealed[128], opened[128] = {};
  const int sealedLength = mesh::Utils::encryptThenMAC(ours, sealed, plain, 19);
  const int openedLength = mesh::Utils::MACThenDecrypt(theirs, opened, sealed, sealedLength);
  const bool seal = openedLength >= 19 && memcmp(opened, plain, 19) == 0;
  uint8_t signature[SIGNATURE_SIZE];
  us.sign(signature, plain, 19);
  const bool signature_ok = mesh::Identity(us.pub_key).verify(signature, plain, 19);
  printf("shared secret agrees: %s\nsealed %d bytes, opened %d: %s\nsignature verifies: %s\n", secret ? "yes" : "NO", sealedLength,
         openedLength, seal ? "yes" : "NO", signature_ok ? "yes" : "NO");
  return secret && seal && signature_ok ? 0 : 1;
}

int main(int argc, char** argv) {
#ifdef _WIN32
  SetConsoleOutputCP(CP_UTF8);
#endif
  setvbuf(stdout, nullptr, _IONBF, 0);

  std::string port = "COM9", tcp, data = "data";
  std::vector<std::string> names;
  bool verbose = false, service = false;
  LoRaParams lp;
  for (int i = 1; i < argc; i++) {
    std::string a = argv[i];
    auto next = [&]() { return i + 1 < argc ? std::string(argv[++i]) : std::string(); };
    if (a == "--port") port = next();
    else if (a == "--tcp") tcp = next();
    else if (a == "--node") names.push_back(next());
    else if (a == "--data") data = next();
    else if (a == "--freq") lp.freq_hz = (uint32_t)(std::stod(next()) * 1e6 + 0.5);
    else if (a == "--bw") lp.bw_hz = (uint32_t)(std::stod(next()) * 1e3 + 0.5);
    else if (a == "--sf") lp.sf = (uint8_t)std::stoi(next());
    else if (a == "--cr") lp.cr = (uint8_t)std::stoi(next());
    else if (a == "--power") lp.tx_power = (int8_t)std::stoi(next());
    else if (a == "--verbose") verbose = true;
    else if (a == "--selftest") return selfTest();
    else if (a == "--service") service = true;
    else { printf("unknown arg %s\n", a.c_str()); return 1; }
  }
  if (service && (tcp.empty() || !names.empty())) {
    fputs("Service mode requires --tcp and uses person addresses instead of --node.\n", stderr);
    return 1;
  }
  if (!service && names.empty()) names.push_back("PC-Node");

  KissHub hub;
  // MAARSE_WORKER_VERBOSE=1 in the service environment logs every frame to and from the modem.
  hub.verbose = verbose || std::getenv("MAARSE_WORKER_VERBOSE") != nullptr;
  if (!(tcp.empty() ? hub.open(port, lp) : hub.openTcp(tcp, lp))) return 1;

  SystemRNG rng;
  SystemRTC rtc;
  std::vector<std::unique_ptr<VirtualRadio>> radios;
  std::vector<std::unique_ptr<VirtualNode>> nodes;
  std::map<std::string, size_t> people;
  std::filesystem::create_directories(data);
  const auto addNode = [&](const std::string& folder, const std::string& name) {
    auto radio = std::make_unique<VirtualRadio>(hub);
    auto node = std::make_unique<VirtualNode>(*radio, rng, rtc);
    if (!node->begin((std::filesystem::path(data) / folder).string(), name.c_str(), service)) return false;
    hub.attach(radio.get());
    radios.push_back(std::move(radio));
    nodes.push_back(std::move(node));
    return true;
  };
  if (service) {
    for (const auto& entry : std::filesystem::directory_iterator(data)) {
      const auto person = entry.path().filename().string();
      if (personAddress(person) && entry.is_directory() && std::filesystem::exists(entry.path() / "identity")) {
        if (!addNode(person, "MeshCore-" + person.substr(0, 8))) {
          fprintf(stderr, "Cannot load MeshCore identity for %s\n", person.c_str());
          return 1;
        }
        people.emplace(person, nodes.size() - 1);
      }
    }
    puts("MeshCore worker ready.");
  } else {
    for (const auto& name : names) {
      if (!addNode(name, name)) {
        fprintf(stderr, "Cannot load or save node %s\n", name.c_str());
        return 1;
      }
      nodes.back()->showWelcome();
    }
  }
  size_t active = 0;
  if (!service) printf("Active node: %s   (type 'help')\n", nodes[active]->name());

  std::thread([service] {
    std::string line;
    while (std::getline(std::cin, line)) {
      if (!line.empty() && line.back() == '\r') line.pop_back();
      if (line.rfind("\xEF\xBB\xBF", 0) == 0) line.erase(0, 3);  // strip UTF-8 BOM (PowerShell pipes)
      std::lock_guard<std::mutex> l(g_mx);
      g_lines.push(line);
    }
    // Under a service manager (systemd) stdin is closed from the start: a service
    // keeps running. Interactively, the end of the input ends the program.
    if (service) return;
    std::lock_guard<std::mutex> l(g_mx);
    g_lines.push("quit");
  }).detach();

  auto nextProvisionCheck = std::chrono::steady_clock::now();
  for (;;) {
    if (service && std::chrono::steady_clock::now() >= nextProvisionCheck) {
      nextProvisionCheck += std::chrono::milliseconds(250);
      std::error_code error;
      for (std::filesystem::directory_iterator entry(data, error), end; !error && entry != end; entry.increment(error)) {
        const auto folder = entry->path();
        const auto person = folder.filename().string();
        const auto request = folder / "provision";
        std::error_code itemError;
        if (!personAddress(person) || !entry->is_directory(itemError)) continue;
        auto existing = people.find(person);
        if (existing == people.end()) {
          if (!std::filesystem::is_regular_file(request, itemError)) continue;
          if (!addNode(person, "MeshCore-" + person.substr(0, 8))) continue;
          existing = people.emplace(person, nodes.size() - 1).first;
        }
        auto& node = *nodes[existing->second];
        node.reloadChannels();
        const auto admin = folder / "repeater-job.txt";
        if (std::filesystem::is_regular_file(admin, itemError)) {
          std::ifstream input(admin, std::ios::binary);
          std::string operation, key, value;
          std::getline(input, operation);
          std::getline(input, key);
          std::getline(input, value);
          input.close();
          if (std::filesystem::remove(admin, itemError) && (operation == "repeater-login" || operation == "repeater-command") && value.size() <= 60) {
            node.repeaterRequest(operation == "repeater-login", key, value.c_str());
          }
        }
        const auto channel = folder / "channel-send.txt";
        if (std::filesystem::is_regular_file(channel, itemError)) {
          std::ifstream input(channel, std::ios::binary);
          std::string name, text;
          std::getline(input, name);
          std::getline(input, text);
          input.close();
          if (!name.empty() && !text.empty()) node.sendNamedChannel(name, text.c_str());
          std::filesystem::remove(channel, itemError);
        }
        if (std::filesystem::is_regular_file(request, itemError) && publishPublicKey(folder, node)) {
          std::filesystem::remove(request, itemError);
        }
        // The person's Maarse name becomes the node's name: name.txt holds it as plain text.
        const auto rename = folder / "name.txt";
        if (std::filesystem::is_regular_file(rename, itemError)) {
          std::ifstream input(rename, std::ios::binary);
          std::string wanted;
          std::getline(input, wanted);
          input.close();
          std::filesystem::remove(rename, itemError);
          if (!wanted.empty()) node.renameTo(wanted.c_str());
        }
        if (std::filesystem::remove(folder / "announce", itemError)) node.sendSelfAdvert(false);
        if (std::filesystem::is_regular_file(folder / "scan", itemError)) node.startServiceScan();
        const auto send = folder / "public-send.txt";
        if (std::filesystem::is_regular_file(send, itemError)) {
          std::ifstream input(send, std::ios::binary);
          char text[91]{};
          input.read(text, sizeof(text));
          const auto length = input.gcount();
          input.close();
          if (length > 0 && length <= 90 && node.sendPublic(text)) std::filesystem::remove(send, itemError);
        }
        const auto direct = folder / "direct-send.txt";
        if (std::filesystem::is_regular_file(direct, itemError)) {
          std::ifstream input(direct, std::ios::binary);
          char record[125]{};
          input.read(record, sizeof(record));
          const auto length = input.gcount();
          input.close();
          if (length > 64 && length < sizeof(record)) node.sendDirect(std::string(record, 64), record + 64);
          std::filesystem::remove(direct, itemError);
        }
      }
    }
    hub.loop();
    for (auto& n : nodes) n->loop();

    std::string cmd; bool have = false;
    {
      std::lock_guard<std::mutex> l(g_mx);
      if (!g_lines.empty()) { cmd = g_lines.front(); g_lines.pop(); have = true; }
    }
    if (have && !cmd.empty()) {
      if (cmd == "quit" || cmd == "exit") break;
      else if (service) {
        const auto space = cmd.find(' ');
        const auto operation = cmd.substr(0, space);
        const auto personEnd = space == std::string::npos ? space : cmd.find(' ', space + 1);
        const auto person = space == std::string::npos ? "" : cmd.substr(space + 1, personEnd - space - 1);
        if (!personAddress(person)) {
          fputs("Invalid MeshCore person address.\n", stderr);
          continue;
        }
        auto existing = people.find(person);
        if (operation == "ensure" && personEnd == std::string::npos) {
          if (existing == people.end()) {
            if (!addNode(person, "MeshCore-" + person.substr(0, 8))) {
              fprintf(stderr, "Cannot provision MeshCore identity for %s\n", person.c_str());
              continue;
            }
            existing = people.emplace(person, nodes.size() - 1).first;
          }
          printProvisioned(person, *nodes[existing->second]);
        } else if (operation == "node" && personEnd != std::string::npos && existing != people.end()) {
          nodes[existing->second]->handleCommand(cmd.c_str() + personEnd + 1);
        } else {
          fputs("Unsupported MeshCore worker command.\n", stderr);
        }
      } else if (cmd == "help") help();
      else if (cmd == "nodes") {
        for (size_t i = 0; i < nodes.size(); i++) printf(" %c %zu: %s\n", i == active ? '*' : ' ', i, nodes[i]->name());
      } else if (cmd.rfind("use ", 0) == 0) {
        std::string w = cmd.substr(4);
        for (size_t i = 0; i < nodes.size(); i++)
          if (w == nodes[i]->name() || w == std::to_string(i)) active = i;
        printf("Active node: %s\n", nodes[active]->name());
      } else if (cmd == "stats") {
        printf("modem: rx=%u tx=%u tx_fail=%u\n", hub.rx_count, hub.tx_count, hub.tx_fail);
      } else {
        nodes[active]->handleCommand(cmd.c_str());
      }
    }
    portable::sleepMs(2);
  }
  hub.close();
  return 0;
}
