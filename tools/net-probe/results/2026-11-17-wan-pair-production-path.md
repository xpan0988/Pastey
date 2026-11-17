# Shaped two-Host runs, 2026-11-17 (single Mac, emulated network)

Builds: before = 2214d79 (neither step), step1 = 19621ac, step1+2 = 77607a9, step1+2+abort = 77607a9 plus the in-flight upload abort; the same test files in each. Run 1 is every scenario; run 2 repeats the three that separate the builds; run 3 is every scenario on the fixed build. Source: `src-tauri/src/connectivity/wan_pair_tests.rs`.

## Run 1

| scenario | build | done | s | first progress s | longest stall s | copies | .part | active s/r | invalidations | max RTT ms | error / extra |
|---|---|---|---|---|---|---|---|---|---|---|---|
| below_floor_direct_64k_16mib | before | NO | 488.26 | – | 0.0 | 0 | 0 | 0/0 | 0 | 416.79 | Transfer interrupted |
| below_floor_direct_64k_16mib | step1 | NO | 485.34 | – | 0.0 | 0 | 0 | 0/0 | 0 | 396.21 | Transfer interrupted |
| below_floor_direct_64k_16mib | step1+2 | NO | 484.84 | – | 0.0 | 0 | 0 | 0/0 | 0 | 421.77 | Transfer interrupted |
| congested_relay_128k_150ms_16mib | before | NO | 135.77 | – | 0.0 | 0 | 0 | 0/0 | mac@37.12s→Reconnecting; windows@37.37s→Reconnecting | 18686.39 | Transfer cancelled |
| congested_relay_128k_150ms_16mib | step1 | NO | 74.03 | – | 0.0 | 0 | 0 | 0/0 | mac@39.21s→Reconnecting; windows@39.73s→Reconnecting | 18627.05 | Transfer cancelled |
| congested_relay_128k_150ms_16mib | step1+2 | yes | 160.78 | 85.72 | 85.72 | 1 | 0 | 0/0 | 0 | 16171.23 |  |
| congested_relay_384k_150ms_32mib | before | NO | 100.25 | – | 0.0 | 0 | 0 | 0/0 | mac@77.23s→Reconnecting; windows@78.25s→Reconnecting | 8516.22 | Transfer cancelled |
| congested_relay_384k_150ms_32mib | step1 | yes | 106.6 | 25.88 | 28.75 | 1 | 0 | 0/0 | 0 | 6423.74 |  |
| congested_relay_384k_150ms_32mib | step1+2 | yes | 103.72 | 26.04 | 26.04 | 1 | 0 | 0/0 | 0 | 5526.78 |  |
| continuation_beside_relay_384k_transfer_16mib | before | yes | 55.87 | 55.22 | 55.22 | 1 | 0 | 0/0 | 0 | 10988.61 | {"continuation_asked_at_s": 7.78, "continuation_queued": {"Ok": null}, "task_end_state": "Completed", "turn_starts": 1} |
| continuation_beside_relay_384k_transfer_16mib | step1 | yes | 56.76 | 30.71 | 30.71 | 1 | 0 | 0/0 | 0 | 6440.61 | {"continuation_asked_at_s": 7.79, "continuation_queued": {"Ok": null}, "task_end_state": "Completed", "turn_starts": 1} |
| continuation_beside_relay_384k_transfer_16mib | step1+2 | yes | 56.9 | 30.7 | 30.7 | 1 | 0 | 0/0 | 0 | 6121.51 | {"continuation_asked_at_s": 7.77, "continuation_queued": {"Ok": null}, "task_end_state": "Completed", "turn_starts": 1} |
| long_outage_45s_direct_384k_16mib | before | NO | 38.09 | – | 0.0 | 0 | 0 | 0/0 | mac@37.3s→Reconnecting; windows@37.3s→Reconnecting | 345.39 | Transfer cancelled {"outage_at_s": 8.0, "outage_s": 45.0} |
| long_outage_45s_direct_384k_16mib | step1 | NO | 38.09 | – | 0.0 | 0 | 0 | 0/0 | mac@37.47s→Reconnecting; windows@37.47s→Reconnecting | 347.68 | Transfer cancelled {"outage_at_s": 8.0, "outage_s": 45.0} |
| long_outage_45s_direct_384k_16mib | step1+2 | NO | 38.39 | – | 0.0 | 0 | 0 | 0/0 | mac@39.28s→Reconnecting; windows@39.54s→Reconnecting | 346.93 | Transfer cancelled {"outage_at_s": 8.0, "outage_s": 45.0} |
| long_outage_after_reconnect_1mib | before | yes | 0.4 | 0.36 | 0.36 | 1 | 0 | 0/0 | mac@37.3s→Reconnecting; windows@37.3s→Reconnecting | 345.39 |  |
| long_outage_after_reconnect_1mib | step1 | yes | 0.38 | 0.33 | 0.33 | 1 | 0 | 0/0 | mac@37.47s→Reconnecting; windows@37.47s→Reconnecting | 347.68 |  |
| long_outage_after_reconnect_1mib | step1+2 | yes | 0.38 | 0.34 | 0.34 | 1 | 0 | 0/0 | mac@39.28s→Reconnecting; windows@39.54s→Reconnecting | 346.93 |  |
| long_outage_recovery | before | | | | | | | | | | {"after_transfer_error": null, "reconnected_after_network_back_s": 0.21} |
| long_outage_recovery | step1 | | | | | | | | | | {"after_transfer_error": null, "reconnected_after_network_back_s": 0.2} |
| long_outage_recovery | step1+2 | | | | | | | | | | {"after_transfer_error": null, "reconnected_after_network_back_s": 0.26} |
| lossy_direct_1pct_100ms_16mib | before | yes | 106.02 | 104.97 | 104.97 | 1 | 0 | 0/0 | 0 | 209.21 |  |
| lossy_direct_1pct_100ms_16mib | step1 | yes | 121.41 | 64.47 | 64.47 | 1 | 0 | 0/0 | 0 | 223.52 |  |
| lossy_direct_1pct_100ms_16mib | step1+2 | yes | 98.34 | 42.01 | 42.01 | 1 | 0 | 0/0 | 0 | 308.14 |  |
| receiver_burn_at_20s_direct_256k_32mib | before | NO | 125.74 | – | 0.0 | 0 | 0 | 0/0 | mac@31.29s→Reconnecting; mac@43.34s→Disconnected; mac@45.41s→Reconnecting; mac@50.3s→Disconnected; mac@52.37s→Reconnecting; mac@57.51s→Disconnected; mac@59.32s→Reconnecting; mac@64.44s→Disconnected; mac@66.48s→Reconnecting; mac@71.35s→Disconnected; mac@73.4s→Reconnecting; mac@78.53s→Disconnected; mac@80.59s→Reconnecting; mac@85.46s→Disconnected; mac@87.52s→Reconnecting; mac@92.68s→Disconnected; mac@94.48s→Reconnecting; mac@99.63s→Disconnected; mac@101.69s→Reconnecting; mac@106.53s→Disconnected; mac@108.58s→Reconnecting; mac@113.69s→Disconnected; mac@115.73s→Reconnecting; mac@120.64s→Disconnected; mac@122.7s→Reconnecting; mac@127.61s→Disconnected | 398.7 | Transfer cancelled {"burned_at_s": 22.42} |
| receiver_burn_at_20s_direct_256k_32mib | step1 | NO | 34.76 | – | 0.0 | 0 | 0 | 0/0 | mac@31.01s→Reconnecting | 398.37 | Transfer cancelled {"burned_at_s": 22.4} |
| receiver_burn_at_20s_direct_256k_32mib | step1+2 | NO | 34.77 | – | 0.0 | 0 | 0 | 0/0 | 0 | 398.63 | Peer disconnected {"burned_at_s": 22.4} |
| sender_burn_at_20s_direct_256k_32mib | before | NO | 125.44 | – | 0.0 | 0 | 0 | 0/0 | windows@31.3s→Reconnecting; windows@43.37s→Disconnected; windows@45.41s→Reconnecting; windows@50.27s→Disconnected; windows@52.32s→Reconnecting; windows@57.43s→Disconnected; windows@59.51s→Reconnecting; windows@64.41s→Disconnected; windows@66.48s→Reconnecting; windows@71.39s→Disconnected; windows@73.45s→Reconnecting; windows@78.62s→Disconnected; windows@80.42s→Reconnecting; windows@85.56s→Disconnected; windows@87.62s→Reconnecting; windows@92.5s→Disconnected; windows@94.57s→Reconnecting; windows@99.7s→Disconnected; windows@101.52s→Reconnecting; windows@106.68s→Disconnected; windows@108.73s→Reconnecting; windows@113.59s→Disconnected; windows@115.64s→Reconnecting; windows@120.75s→Disconnected; windows@122.8s→Reconnecting; windows@127.69s→Disconnected | 398.17 | Transfer cancelled {"burned_at_s": 22.38, "sender_stopped_after_burn_s": 105.44} |
| sender_burn_at_20s_direct_256k_32mib | step1 | NO | 34.58 | – | 0.0 | 0 | 0 | 0/0 | windows@31.28s→Reconnecting | 398.89 | Transfer cancelled {"burned_at_s": 22.42, "receiver_clean_after_burn_s": 14.58, "sender_stopped_after_burn_s": 14.58} |
| sender_burn_at_20s_direct_256k_32mib | step1+2 | NO | 35.36 | – | 0.0 | 0 | 0 | 0/0 | 0 | 648.49 | Transfer cancelled {"burned_at_s": 22.4, "receiver_clean_after_burn_s": 15.36, "sender_stopped_after_burn_s": 15.36} |
| short_outage_10s_relay_384k_16mib | before | yes | 61.29 | 60.76 | 60.76 | 1 | 0 | 0/0 | 0 | 13700.78 | {"outage_at_s": 10.0, "outage_s": 10.0} |
| short_outage_10s_relay_384k_16mib | step1 | yes | 62.69 | 35.86 | 35.86 | 1 | 0 | 0/0 | 0 | 13668.97 | {"outage_at_s": 10.0, "outage_s": 10.0} |
| short_outage_10s_relay_384k_16mib | step1+2 | yes | 62.56 | 35.69 | 35.69 | 1 | 0 | 0/0 | 0 | 13652.02 | {"outage_at_s": 10.0, "outage_s": 10.0} |
| short_outage_15s_direct_384k_16mib | before | yes | 67.17 | 66.16 | 66.16 | 1 | 0 | 0/0 | 0 | 347.53 | {"outage_at_s": 10.0, "outage_s": 15.0} |
| short_outage_15s_direct_384k_16mib | step1 | yes | 68.26 | 42.92 | 42.92 | 1 | 0 | 0/0 | 0 | 346.62 | {"outage_at_s": 10.0, "outage_s": 15.0} |
| short_outage_15s_direct_384k_16mib | step1+2 | yes | 68.27 | 42.91 | 42.91 | 1 | 0 | 0/0 | 0 | 681.34 | {"outage_at_s": 10.0, "outage_s": 15.0} |
| slow_direct_256k_350ms_32mib | before | NO | 488.87 | – | 0.0 | 0 | 0 | 0/0 | 0 | 599.46 | Transfer interrupted |
| slow_direct_256k_350ms_32mib | step1 | yes | 146.97 | 36.43 | 36.43 | 1 | 0 | 0/0 | 0 | 956.35 |  |
| slow_direct_256k_350ms_32mib | step1+2 | yes | 147.14 | 36.25 | 36.25 | 1 | 0 | 0/0 | 0 | 924.31 |  |

## Run 2

| scenario | build | done | s | first progress s | longest stall s | copies | .part | active s/r | invalidations | max RTT ms | error / extra |
|---|---|---|---|---|---|---|---|---|---|---|---|
| congested_relay_128k_150ms_16mib | before | NO | 136.21 | – | 0.0 | 0 | 0 | 0/0 | mac@37.57s→Reconnecting; windows@37.82s→Reconnecting | 19287.32 | Transfer cancelled |
| congested_relay_128k_150ms_16mib | step1 | NO | 83.18 | – | 0.0 | 0 | 0 | 0/0 | windows@34.23s→Reconnecting; mac@41.2s→Reconnecting | 17933.32 | Transfer cancelled |
| congested_relay_128k_150ms_16mib | step1+2 | yes | 153.55 | 77.75 | 77.75 | 1 | 0 | 0/0 | 0 | 14701.08 |  |
| congested_relay_384k_150ms_32mib | before | NO | 100.27 | – | 0.0 | 0 | 0 | 0/0 | mac@59.88s→Reconnecting; windows@60.14s→Reconnecting | 8431.83 | Transfer cancelled |
| congested_relay_384k_150ms_32mib | step1 | yes | 106.1 | 26.02 | 27.59 | 1 | 0 | 0/0 | 0 | 6264.55 |  |
| congested_relay_384k_150ms_32mib | step1+2 | yes | 106.66 | 26.08 | 28.05 | 1 | 0 | 0/0 | 0 | 6322.29 |  |
| slow_direct_256k_350ms_32mib | before | NO | 488.75 | – | 0.0 | 0 | 0 | 0/0 | 0 | 1012.99 | Transfer interrupted |
| slow_direct_256k_350ms_32mib | step1 | yes | 148.57 | 36.71 | 36.71 | 1 | 0 | 0/0 | 0 | 876.3 |  |
| slow_direct_256k_350ms_32mib | step1+2 | yes | 148.56 | 36.53 | 36.53 | 1 | 0 | 0/0 | 0 | 905.82 |  |

## Run 3: step1+2 plus the in-flight abort fix

| scenario | build | done | s | first progress s | longest stall s | copies | .part | active s/r | invalidations | max RTT ms | error / extra |
|---|---|---|---|---|---|---|---|---|---|---|---|
| below_floor_direct_64k_16mib | step1+2+abort | NO | 483.83 | – | 0.0 | 0 | 0 | 0/0 | 0 | 388.43 | Transfer interrupted |
| congested_relay_128k_150ms_16mib | step1+2+abort | yes | 151.24 | 76.67 | 76.67 | 1 | 0 | 0/0 | 0 | 14965.1 |  |
| congested_relay_384k_150ms_32mib | step1+2+abort | yes | 103.34 | 25.13 | 28.27 | 1 | 0 | 0/0 | 0 | 6506.67 |  |
| continuation_beside_relay_384k_transfer_16mib | step1+2+abort | yes | 55.12 | 30.09 | 30.09 | 1 | 0 | 0/0 | 0 | 4043.42 | {"continuation_asked_at_s": 7.26, "continuation_queued": {"Ok": null}, "task_end_state": "Completed", "turn_starts": 1} |
| long_outage_45s_direct_384k_16mib | step1+2+abort | NO | 38.38 | – | 0.0 | 0 | 0 | 0/0 | mac@38.72s→Reconnecting | 346.65 | Transfer cancelled {"outage_at_s": 8.0, "outage_s": 45.0} |
| long_outage_after_reconnect_1mib | step1+2+abort | yes | 0.22 | 0.2 | 0.2 | 1 | 0 | 0/0 | mac@38.72s→Reconnecting; windows@54.46s→Reconnecting | 346.65 |  |
| long_outage_recovery | step1+2+abort | | | | | | | | | | {"after_transfer_error": null, "reconnected_after_network_back_s": 0.26} |
| lossy_direct_1pct_100ms_16mib | step1+2+abort | yes | 98.61 | 43.73 | 43.73 | 1 | 0 | 0/0 | 0 | 222.86 |  |
| receiver_burn_at_20s_direct_256k_32mib | step1+2+abort | NO | 34.11 | – | 0.0 | 0 | 0 | 0/0 | 0 | 398.32 | Peer disconnected {"burned_at_s": 21.24} |
| sender_burn_at_20s_direct_256k_32mib | step1+2+abort | NO | 20.04 | – | 0.0 | 0 | 0 | 0/0 | 0 | 398.33 | Transfer cancelled {"burned_at_s": 21.23, "receiver_clean_after_burn_s": 16.9, "sender_stopped_after_burn_s": 0.05} |
| short_outage_10s_relay_384k_16mib | step1+2+abort | yes | 60.46 | 34.91 | 34.91 | 1 | 0 | 0/0 | 0 | 14744.76 | {"outage_at_s": 10.0, "outage_s": 10.0} |
| short_outage_15s_direct_384k_16mib | step1+2+abort | yes | 66.59 | 42.51 | 42.51 | 1 | 0 | 0/0 | 0 | 354.75 | {"outage_at_s": 10.0, "outage_s": 15.0} |
| slow_direct_256k_350ms_32mib | step1+2+abort | yes | 143.33 | 35.42 | 35.42 | 1 | 0 | 0/0 | 0 | 808.68 |  |
