# zbus 5.19.0, with two small patches

An unmodified copy of zbus 5.19.0 from crates.io (MIT, see LICENSE), except for the two changes below.
Search for "Lull patch".

**Why:** any local program can send messages to Lull, and Lull's rule is always under 50 MB. The tray
library (ksni) and Slint open their own connections, so neither limit can be set from Lull's code.

1. `src/connection/socket/mod.rs`: zbus read messages of up to 128 MB into memory (a 16 MB one took Lull to 38.6 MB,
   for good). Now anything over 32 KB is read through a 16 KB buffer and dropped, and the connection stays up. Every
   message Lull really uses is under 13 KB.
2. `src/object_server/mod.rs`: zbus started a task, holding the message, for every incoming call without limit
   (800 calls of 120 KB took Lull to 110 MB). Now 16 run at once, and a flood waits in the socket.

**Consequence:** a reply over 32 KB never arrives, so don't call methods that return more from this process.

**Updating zbus:** copy the new version here, make both changes, and run `scripts/desktop-e2e.sh`, which sends huge
and flooding messages. Remove each patch if zbus gains the option.
