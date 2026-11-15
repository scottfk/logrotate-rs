# Upstream quirks, kept on purpose

logrotate-rs reproduces logrotate **3.22.0**, as Gentoo builds it, exactly, including
behavior that looks like a bug. Each item here is marked `QUIRK` in the source. Fixing
one is a deliberate, separate change (ideally upstream as well), not something to slip
into the port.

Severity: **high** means it can lose or expose log data, or touch the wrong file;
**medium** means it's surprising and user-visible; **low** means it's cosmetic or hard
to hit.

Nothing is listed yet: the port hasn't started.
