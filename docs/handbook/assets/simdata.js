/* Measured simulator results used by the handbook charts. Generated from the raw runs; see simulation.html for provenance. */
window.SIM = {
 "rolesForest": [
  {
   "name": "All clients",
   "series": 2,
   "delivered": 95.4,
   "acked": 95.4,
   "latency": 1.0,
   "tx": 619,
   "airtime": 248
  },
  {
   "name": "All relays",
   "series": 1,
   "delivered": 95.4,
   "acked": 94.9,
   "latency": 1.1,
   "tx": 1000,
   "airtime": 445
  },
  {
   "name": "Auto (relay after 2 min)",
   "series": 0,
   "delivered": 99.7,
   "acked": 98.9,
   "latency": 1.2,
   "tx": 991,
   "airtime": 438
  }
 ],
 "rolesCity": [
  {
   "name": "All clients",
   "series": 2,
   "delivered": 88.1,
   "acked": 79.8,
   "latency": 1.6,
   "tx": 6139,
   "airtime": 2519
  },
  {
   "name": "All relays",
   "series": 1,
   "delivered": 72.8,
   "acked": 18.7,
   "latency": 13.4,
   "tx": 41292,
   "airtime": 19901
  },
  {
   "name": "Auto (relay after 2 min)",
   "series": 0,
   "delivered": 90.2,
   "acked": 73.8,
   "latency": 3.0,
   "tx": 14462,
   "airtime": 6376
  }
 ],
 "now": {
  "national": {
   "honest": {
    "delivered": 84.4,
    "airtime": 44870,
    "latency": 4.1,
    "seeds": 4,
    "by": {
     "Ack": 1143.6,
     "Announce": 10800.3,
     "Channel": 593.6,
     "Data": 71.3,
     "Deposit": 3495.4,
     "Hello": 1581.5,
     "KeyRequest": 58.4,
     "MailSync": 279.8,
     "PathRequest": 24107.5,
     "PathResponse": 2786.6
    }
   },
   "c_old": {
    "delivered": 85.3,
    "airtime": 49822,
    "latency": 3.9,
    "seeds": 4,
    "by": {
     "Ack": 1220.4,
     "Announce": 10556.8,
     "Channel": 612.9,
     "Data": 40.3,
     "Deposit": 4221.9,
     "Hello": 1601.5,
     "KeyRequest": 44.6,
     "MailSync": 250.0,
     "PathRequest": 28207.7,
     "PathResponse": 3119.0
    }
   },
   "c_new": {
    "delivered": 86.2,
    "airtime": 44106,
    "latency": 3.4,
    "seeds": 4,
    "by": {
     "Ack": 1066.3,
     "Announce": 10932.6,
     "Channel": 612.3,
     "Data": 28.3,
     "Deposit": 3546.6,
     "Hello": 1562.4,
     "KeyRequest": 51.2,
     "MailSync": 271.1,
     "PathRequest": 23238.7,
     "PathResponse": 2843.9
    }
   }
  },
  "forest": {
   "honest": {
    "delivered": 98.0,
    "airtime": 826,
    "latency": 1.8,
    "seeds": 5,
    "by": {
     "Ack": 13.9,
     "Announce": 168.9,
     "Channel": 50.6,
     "Data": 57.0,
     "Hello": 56.9,
     "KeyRequest": 10.7,
     "PathRequest": 424.3,
     "PathResponse": 44.5
    }
   },
   "c_old": {
    "delivered": 99.6,
    "airtime": 768,
    "latency": 1.4,
    "seeds": 5,
    "by": {
     "Ack": 16.4,
     "Announce": 166.2,
     "Channel": 62.8,
     "Data": 44.9,
     "Hello": 57.4,
     "KeyRequest": 9.3,
     "PathRequest": 363.5,
     "PathResponse": 48.3
    }
   },
   "c_new": {
    "delivered": 98.6,
    "airtime": 732,
    "latency": 1.5,
    "seeds": 5,
    "by": {
     "Ack": 17.5,
     "Announce": 161.2,
     "Channel": 54.2,
     "Data": 46.2,
     "Hello": 56.9,
     "KeyRequest": 11.1,
     "PathRequest": 338.1,
     "PathResponse": 47.1
    }
   }
  },
  "town": {
   "honest": {
    "delivered": 59.4,
    "airtime": 7710,
    "latency": 3.9,
    "seeds": 5,
    "by": {
     "Ack": 74.5,
     "Announce": 1604.8,
     "Channel": 107.0,
     "Data": 24.2,
     "Deposit": 478.0,
     "Hello": 676.9,
     "KeyRequest": 280.6,
     "MailSync": 8.8,
     "PathRequest": 3814.7,
     "PathResponse": 649.5
    }
   },
   "c_old": {
    "delivered": 58.0,
    "airtime": 7754,
    "latency": 3.9,
    "seeds": 5,
    "by": {
     "Ack": 79.4,
     "Announce": 1605.3,
     "Channel": 95.2,
     "Data": 22.5,
     "Deposit": 549.2,
     "Hello": 674.0,
     "KeyRequest": 285.8,
     "MailSync": 7.7,
     "PathRequest": 3719.1,
     "PathResponse": 725.1
    }
   },
   "c_new": {
    "delivered": 59.9,
    "airtime": 7569,
    "latency": 3.4,
    "seeds": 5,
    "by": {
     "Ack": 93.9,
     "Announce": 1718.9,
     "Channel": 111.7,
     "Data": 18.7,
     "Deposit": 550.3,
     "Hello": 700.1,
     "KeyRequest": 316.0,
     "MailSync": 10.4,
     "PathRequest": 3342.3,
     "PathResponse": 715.3
    }
   }
  },
  "car": {
   "honest": {
    "delivered": 96.9,
    "airtime": 1285,
    "latency": 4.3,
    "seeds": 7,
    "by": {
     "Ack": 159.1,
     "Announce": 139.8,
     "Carry": 5.7,
     "Data": 417.0,
     "Hello": 165.5,
     "PathRequest": 352.7,
     "PathResponse": 45.1,
     "KeyRequest": 1.3
    }
   },
   "c_old": {
    "delivered": 90.2,
    "airtime": 1770,
    "latency": 5.4,
    "seeds": 7,
    "by": {
     "Ack": 243.6,
     "Announce": 142.1,
     "Data": 523.9,
     "Hello": 164.6,
     "PathRequest": 576.9,
     "PathResponse": 115.3,
     "Carry": 3.8,
     "KeyRequest": 1.9
    }
   },
   "c_new": {
    "delivered": 96.9,
    "airtime": 1322,
    "latency": 3.6,
    "seeds": 7,
    "by": {
     "Ack": 195.7,
     "Announce": 139.9,
     "Carry": 9.2,
     "Data": 399.0,
     "Hello": 165.2,
     "PathRequest": 354.7,
     "PathResponse": 58.3,
     "KeyRequest": 1.3
    }
   }
  },
  "outage": {
   "honest": {
    "delivered": 36,
    "airtime": 89520,
    "latency": 7.5,
    "seeds": 5,
    "by": {
     "Ack": 1648.0,
     "Announce": 12304.0,
     "Channel": 587.7,
     "Data": 28.6,
     "Deposit": 6165.2,
     "Hello": 2724.2,
     "KeyRequest": 597.8,
     "MailSync": 452.0,
     "PathRequest": 58332.4,
     "PathResponse": 6789.2
    }
   },
   "c_old": {
    "delivered": 36.8,
    "airtime": 99313,
    "latency": 6.0,
    "seeds": 5,
    "by": {
     "Ack": 1736.5,
     "Announce": 11673.1,
     "Channel": 486.6,
     "Data": 10.3,
     "Deposit": 6889.5,
     "Hello": 2688.7,
     "KeyRequest": 581.2,
     "MailSync": 431.6,
     "PathRequest": 67939.2,
     "PathResponse": 6993.7
    }
   },
   "c_new": {
    "delivered": 36,
    "airtime": 89655,
    "latency": 12.5,
    "seeds": 5,
    "by": {
     "Ack": 1643.9,
     "Announce": 12158.2,
     "Channel": 611.8,
     "Data": 11.2,
     "Deposit": 6171.7,
     "Hello": 2714.4,
     "KeyRequest": 604.8,
     "MailSync": 432.2,
     "PathRequest": 58518.5,
     "PathResponse": 6898.1
    }
   }
  }
 },
 "chain": {
  "national": {
   "boards before": {
    "delivered": 3.7,
    "airtime": 19779,
    "latency": 0.9
   },
   "chain fixes": {
    "delivered": 91.1,
    "airtime": 37295,
    "latency": 2.4
   },
   "+ queue fix": {
    "delivered": 93.3,
    "airtime": 37833,
    "latency": 2.2
   },
   "+ ring search": {
    "delivered": 95.2,
    "airtime": 32901,
    "latency": 2.0
   },
   "+ compression": {
    "delivered": 96.1,
    "airtime": 31747,
    "latency": 1.7
   },
   "+ announce 3": {
    "delivered": 96.0,
    "airtime": 24484,
    "latency": 1.8
   }
  },
  "forest": {
   "boards before": {
    "delivered": 49.1,
    "airtime": 424,
    "latency": 1.0
   },
   "chain fixes": {
    "delivered": 99.7,
    "airtime": 760,
    "latency": 1.6
   },
   "+ queue fix": {
    "delivered": 99.7,
    "airtime": 763,
    "latency": 1.6
   },
   "+ ring search": {
    "delivered": 98.8,
    "airtime": 747,
    "latency": 1.3
   },
   "+ compression": {
    "delivered": 99.5,
    "airtime": 599,
    "latency": 1.1
   },
   "+ announce 3": {
    "delivered": 99.1,
    "airtime": 630,
    "latency": 1.2
   }
  },
  "town": {
   "boards before": {
    "delivered": 10.7,
    "airtime": 1543,
    "latency": 0.7
   },
   "chain fixes": {
    "delivered": 58.6,
    "airtime": 7532,
    "latency": 3.8
   },
   "+ queue fix": {
    "delivered": 59.8,
    "airtime": 7757,
    "latency": 3.8
   },
   "+ ring search": {
    "delivered": 58.1,
    "airtime": 7654,
    "latency": 3.8
   },
   "+ compression": {
    "delivered": 61.6,
    "airtime": 7601,
    "latency": 3.6
   },
   "+ announce 3": {
    "delivered": 61.3,
    "airtime": 7740,
    "latency": 3.4
   }
  },
  "car": {
   "boards before": {
    "delivered": 67.3,
    "airtime": 880,
    "latency": 2.9
   },
   "chain fixes": {
    "delivered": 70.1,
    "airtime": 2238,
    "latency": 2.8
   },
   "+ queue fix": {
    "delivered": 96.3,
    "airtime": 1486,
    "latency": 3.4
   },
   "+ ring search": {
    "delivered": 96.3,
    "airtime": 1486,
    "latency": 3.4
   },
   "+ compression": {
    "delivered": 96.6,
    "airtime": 1205,
    "latency": 2.6
   },
   "+ announce 3": {
    "delivered": 97.1,
    "airtime": 954,
    "latency": 2.7
   }
  },
  "outage": {
   "boards before": {
    "delivered": 1.3,
    "airtime": 20067,
    "latency": 12.8
   },
   "chain fixes": {
    "delivered": 83.3,
    "airtime": 54970,
    "latency": 210.0
   },
   "+ queue fix": {
    "delivered": 80,
    "airtime": 58664,
    "latency": 303.7
   },
   "+ ring search": {
    "delivered": 93.3,
    "airtime": 51342,
    "latency": 498.0
   },
   "+ compression": {
    "delivered": 98.0,
    "airtime": 48669,
    "latency": 368.4
   },
   "+ announce 3": {
    "delivered": 98.0,
    "airtime": 37425,
    "latency": 138.3
   }
  }
 },
 "chainWhen": "2026-11-16T17:04:21.068Z",
 "journey": {
  "up": [
   {
    "name": "no mail (today)",
    "offline": 4,
    "median": 5,
    "onBornholm": 99,
    "airtime": 44870
   },
   {
    "name": "named post box (old)",
    "offline": 36,
    "median": 33,
    "onBornholm": 100,
    "airtime": 45892
   },
   {
    "name": "post offices (new)",
    "offline": 76,
    "median": 2142,
    "onBornholm": 54,
    "airtime": 112710
   },
   {
    "name": "post offices + repeaters",
    "offline": 88,
    "median": 1603,
    "onBornholm": 67,
    "airtime": 111778
   }
  ],
  "down": [
   {
    "name": "no mail (today)",
    "offline": 4,
    "median": 17,
    "onBornholm": 96,
    "airtime": 36521
   },
   {
    "name": "named post box (old)",
    "offline": 3,
    "median": 37,
    "onBornholm": 93,
    "airtime": 38407
   },
   {
    "name": "post offices (new)",
    "offline": 39,
    "median": 6,
    "onBornholm": 51,
    "airtime": 108670
   },
   {
    "name": "post offices + repeaters",
    "offline": 39,
    "median": 6,
    "onBornholm": 52,
    "airtime": 108307
   }
  ]
 },
 "trunks": [
  {
   "name": "internet up, no trunks",
   "all": 73.7,
   "water": 39.0,
   "median": 205.8,
   "airtime": 57972
  },
  {
   "name": "internet up, trunks (backup)",
   "all": 72.9,
   "water": 47.9,
   "median": 378.0,
   "airtime": 58445
  },
  {
   "name": "internet up, trunks (always)",
   "all": 68.3,
   "water": 43.6,
   "median": 360.1,
   "airtime": 58838
  },
  {
   "name": "internet down, no trunks",
   "all": 68.8,
   "water": 2.5,
   "median": 45.2,
   "airtime": 51914
  },
  {
   "name": "internet down, trunks (all floods)",
   "all": 70.0,
   "water": 10.7,
   "median": 395.6,
   "airtime": 53710
  },
  {
   "name": "internet down, trunks (all requests)",
   "all": 73.2,
   "water": 15.0,
   "median": 415.6,
   "airtime": 53319
  },
  {
   "name": "internet down, trunks (backup)",
   "all": 70.1,
   "water": 11.7,
   "median": 409.9,
   "airtime": 53136
  }
 ],
 "crisis": [
  {
   "name": "no carry",
   "near": 92,
   "mid": 66,
   "far": 9,
   "airtime": 207169,
   "handoffs": 0,
   "carried": 0
  },
  {
   "name": "carry as today",
   "near": 95,
   "mid": 76,
   "far": 14,
   "airtime": 256764,
   "handoffs": 2446,
   "carried": 3
  },
  {
   "name": "+ 6 h, 64 in flash",
   "near": 94,
   "mid": 70,
   "far": 11,
   "airtime": 377694,
   "handoffs": 4554,
   "carried": 57
  },
  {
   "name": "+ 16 copies",
   "near": 94,
   "mid": 70,
   "far": 11,
   "airtime": 382270,
   "handoffs": 6785,
   "carried": 43
  },
  {
   "name": "+ offices hand to carriers",
   "near": 94,
   "mid": 70,
   "far": 11,
   "airtime": 380131,
   "handoffs": 4726,
   "carried": 54
  }
 ]
};
