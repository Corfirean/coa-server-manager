---
area: manager
type: fixed
audience: admins
title: Deploying companions shows why the server refused, instead of a bar stuck at 0
---
When the bot module refused a Deploy request, the page kept showing "Creating bots 0 / N" forever. It now shows the module's own reply, and the message for an outdated bot module points to the Modules page.
