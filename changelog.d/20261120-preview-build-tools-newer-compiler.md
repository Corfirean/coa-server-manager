---
area: manager
type: fixed
audience: admins
title: Installing the C++ Build Tools now gets the exact compiler the core needs
---
Plugins must be built by the same compiler build as the core. On a PC whose Visual Studio Build Tools were installed earlier, the installer kept the older compiler and the modules stayed switched off. When the existing Build Tools cannot supply the compiler this core names, the Manager now installs Microsoft's current Build Tools next to them and leaves the existing installation untouched.
