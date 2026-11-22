# ksni 0.3.7, with one small patch

An unmodified copy of ksni 0.3.7 from crates.io (Unlicense), except for the change below. Search for "Lull patch".

**Why:** `ProvideXdgActivationToken` put every token on an unbounded queue until the next click. Any local program
could grow it without end: 30,000 calls took Lull to 144 MB. Lull's rule is always under 50 MB.

**The change:** in `src/dbus_interface.rs`, tokens over 256 bytes are ignored and at most 4 are kept.

**Updating ksni:** copy the new version here, make the same change, and run `scripts/desktop-e2e.sh`. Remove the patch
if ksni bounds that queue itself.
