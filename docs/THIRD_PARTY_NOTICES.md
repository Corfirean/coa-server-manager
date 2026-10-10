# Third-Party Notices and Licenses

## 1. Project Distribution License

**CoA Server Manager** is distributed under the **GNU Affero General Public License Version 3** (`AGPL-3.0-only`).

```text
CoA Server Manager — AzerothCore / Conquest of Azeroth Server Management & Portable Characters
Copyright (C) 2026 CoA Project Contributors

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published by
the Free Software Foundation, version 3 of the License.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program. If not, see <https://www.gnu.org/licenses/>.
```

The complete license text is available in the [`LICENSE`](../LICENSE) file in the repository root.

---

## 2. Rust Application Dependencies

The desktop manager binary (`coa-server-manager.exe`) and workspace crates (`coa-core`, `coa-registry-proto`, `coa-control-proto`) incorporate the following third-party Rust libraries from `Cargo.lock`:

| Package | License | Summary / Purpose |
|---|---|---|
| `tauri` | MIT OR Apache-2.0 | Desktop application runtime and webview window management |
| `tauri-plugin-dialog` | MIT OR Apache-2.0 | Native OS file picker and alert dialog integration |
| `tauri-plugin-process` | MIT OR Apache-2.0 | Desktop process termination and relaunch |
| `tauri-plugin-updater` | MIT OR Apache-2.0 | Passive in-app update checks and installer triggering |
| `tauri-plugin-opener` | MIT OR Apache-2.0 | OS default browser and path opener |
| `serde` / `serde_json` | MIT OR Apache-2.0 | Serialization and deserialization framework |
| `rusqlite` | MIT | Embedded SQLite database access (`portable.sqlite`, `control.sqlite`) |
| `reqwest` | MIT OR Apache-2.0 | HTTP client for Registry interaction and update downloads |
| `tungstenite` | MIT OR Apache-2.0 | WebSocket client runtime for Coordinator and Relay tunnels |
| `snow` | Apache-2.0 OR MIT | Noise Protocol Framework (`Noise_XX_25519_ChaChaPoly_BLAKE2s`) |
| `ed25519-dalek` | BSD-3-Clause | Ed25519 digital signatures for realm advertisements and host authentication |
| `sha2` / `sha1` | MIT OR Apache-2.0 | Cryptographic SHA-256 and SHA-1 hashing |
| `hex` | MIT OR Apache-2.0 | Hexadecimal encoding and decoding |
| `base64` | MIT OR Apache-2.0 | Base64 encoding and decoding |
| `uuid` | Apache-2.0 OR MIT | Universally unique identifiers (v4 random, v7 timestamp-ordered) |
| `chrono` | MIT OR Apache-2.0 | Date and time manipulation and RFC 3339 serialization |
| `zstd` | MIT | Zstandard high-performance compression for portable snapshots and SQL dumps |
| `zip` | MIT | Diagnostic package creation and updater archive decompression |
| `tar` | MIT OR Apache-2.0 | Tar archive extraction for server builds and assets |
| `num-bigint` | MIT OR Apache-2.0 | Big integer calculations for SRP-6 authentication |
| `tracing` / `tracing-subscriber` | MIT | Structured diagnostics logging and redaction filtering |
| `fs4` | MIT OR Apache-2.0 | Cross-platform file locking for data stores |
| `dunce` | CC0-1.0 OR MIT-0 OR Apache-2.0 | Windows UNC path normalization |
| `if-addrs` | MIT OR Apache-2.0 | Local network interface address discovery |
| `thiserror` | MIT OR Apache-2.0 | Structured error derivation |
| `windows-sys` | MIT OR Apache-2.0 | Windows API bindings (DPAPI `CryptProtectData`, IP helper, sockets) |

*(Note: UPnP IGD and NAT-PMP protocols are implemented natively within `coa-core` using raw UDP/SSDP sockets; external crates `natpmp` and `igd_next` are not part of the release dependency graph).*

---

## 3. Frontend (React) Dependencies

The user interface is built with **React 19** and bundled with Vite. Dependencies from `package.json` and `package-lock.json` are licensed as follows:

| Package | Version | License | Summary / Purpose |
|---|---|---|---|
| `react` | 19.3.0 | MIT | Declarative component UI framework |
| `react-dom` | 19.3.0 | MIT | React rendering backend for the webview DOM |
| `@tauri-apps/api` | 2.12.0 | Apache-2.0 OR MIT | Frontend IPC bindings to Tauri core |
| `@tauri-apps/plugin-dialog` | 2.8.0 | MIT OR Apache-2.0 | Dialog API frontend bindings |
| `@tauri-apps/plugin-process` | 2.4.0 | MIT OR Apache-2.0 | Process API frontend bindings |
| `@tauri-apps/plugin-updater` | 2.13.1 | MIT OR Apache-2.0 | In-app updater frontend bindings |
| `lucide-react` | 1.49.0 | ISC | Application iconography |
| `clsx` | 2.1.1 | MIT | Dynamic class string concatenation |
| `tailwind-merge` | 3.7.0 | MIT | Tailwind CSS class collision resolution |
| `class-variance-authority`| 0.7.1 | Apache-2.0 | Type-safe component styling variants |
| `vite` | 8.3.1 | MIT | Build tool and module bundler |
| `@vitejs/plugin-react` | 6.1.1 | MIT | React fast refresh and JSX transformation |
| `tailwindcss` | 4.3.3 | MIT | Modern utility-first CSS styling engine |
| `@tailwindcss/vite` | 4.3.3 | MIT | Vite plugin for Tailwind CSS v4 |
| `typescript` | 7.0.2 | Apache-2.0 | Static type checking and transpilation |

---

## 4. Standard License Texts

### MIT License
```text
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Apache License 2.0
```text
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```

### BSD 3-Clause License
```text
Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### ISC License
```text
Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```
