---
area: manager
type: fixed
audience: admins
title: Update checks no longer download a server archive to compare its launcher
---
Manager recognizes its supported launcher edits against the signed file hash locally. Overlapping checks wait briefly for one another, and the private update copy checks combined program and database space before copying.
