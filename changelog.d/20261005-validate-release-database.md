---
area: manager
type: fixed
audience: admins
title: Server packages must include a complete database schema check
---
Release tools generate the expected database structure on an isolated copy of the signed base, verify all archive files and refuse packages without the schema check. Corrective SQL can restore missing Wildcard tables without replaying old updates.
