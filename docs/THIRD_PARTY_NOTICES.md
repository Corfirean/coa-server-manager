# Third-Party Notices and Licenses

CoA Server Manager incorporates open-source software packages under the following licenses.

---

## 1. Rust Workspace Dependencies

| Package | License | Summary / Purpose |
|---|---|---|
| `tauri` | MIT OR Apache-2.0 | Desktop application framework |
| `serde` / `serde_json` | MIT OR Apache-2.0 | Serialization / deserialization |
| `rusqlite` | MIT | Embedded SQLite database access |
| `reqwest` | MIT OR Apache-2.0 | HTTP client runtime |
| `tokio` | MIT | Asynchronous runtime |
| `axum` | MIT | Web service framework (VPS services) |
| `snow` | Apache-2.0 OR MIT | Noise Protocol Framework implementation (`Noise_XX_25519_ChaChaPoly_BLAKE2s`) |
| `ed25519-dalek` | BSD-3-Clause | Ed25519 digital signature algorithm |
| `sha2` | MIT OR Apache-2.0 | SHA-256 cryptographic hashing |
| `base64` | MIT OR Apache-2.0 | Base64 encoding / decoding |
| `hex` | MIT OR Apache-2.0 | Hexadecimal encoding / decoding |
| `uuid` | Apache-2.0 OR MIT | UUIDv4 / UUIDv7 identifiers |
| `chrono` | MIT OR Apache-2.0 | Time and date handling |
| `zip` | MIT | Diagnostic package and archive creation |
| `tungstenite` / `tokio-tungstenite` | MIT OR Apache-2.0 | WebSocket client and server |
| `tracing` / `tracing-subscriber` | MIT | Structured diagnostics and logging |
| `flate2` | MIT OR Apache-2.0 | Gzip and Deflate compression |
| `windows-sys` | MIT OR Apache-2.0 | Windows API bindings (DPAPI, routing table, processes) |
| `natpmp` | MIT | NAT-PMP port mapping protocol |
| `igd_next` | MIT | UPnP IGD port mapping protocol |

---

## 2. Frontend (UI) Dependencies

| Package | License | Summary / Purpose |
|---|---|---|
| `vue` | MIT | Reactive UI framework |
| `pinia` | MIT | State management store |
| `vue-router` | MIT | Frontend client routing |
| `lucide-vue-next` | ISC | Icons library |
| `vite` | MIT | Frontend bundler and build tool |
| `typescript` | Apache-2.0 | Static type checking |

---

## 3. License Texts

### Apache License 2.0
Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except in compliance with the License. You may obtain a copy of the License at `http://www.apache.org/licenses/LICENSE-2.0`.

### MIT License
Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:
The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

### BSD 3-Clause License
Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:
1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

### ISC License
Permission to use, copy, modify, and/or distribute this software for any purpose with or without fee is hereby granted, provided that the above copyright notice and this permission notice appear in all copies.
