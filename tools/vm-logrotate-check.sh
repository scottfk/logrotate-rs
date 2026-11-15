#!/bin/sh
# 20261115 - Created by Claude (claude.ai, Anthropic) - scottfk
#
# Rehearsal check for the Gentoo package, run as root on a test machine.
#
#   vm-logrotate-check.sh setup          make the test tree and save it
#   vm-logrotate-check.sh capture LABEL  restore the tree, run logrotate,
#                                        record everything in RESULTS/LABEL.txt
#   vm-logrotate-check.sh compare A B    diff two captures
#
# A capture runs the installed /usr/bin/logrotate on a test tree (real users,
# su, ACLs, compression, dateext, olddir, scripts, mail, includes), twice:
# once on a state file that says when each log was last rotated, then with
# -f.  It also dry-runs the system's /etc/logrotate.conf (-d) and runs
# /etc/cron.daily/logrotate for real.  Take captures on the same day: the
# dateext names hold the date.
#
# The test user is the first user with uid 1000 (scottfk on the test VM).

set -u
TREE=/var/tmp/lrcheck
SAVE=/var/tmp/lrcheck-saved.tar
RESULTS=/var/tmp/lrcheck-results
U=$(getent passwd 1000 | cut -d: -f1)
[ -n "$U" ] || { echo "no user with uid 1000" >&2; exit 1; }
[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }

old() { touch -d "$1" "$2"; }

setup() {
	rm -rf "$TREE"
	mkdir -p "$TREE"/logs/user "$TREE"/logs/glob "$TREE"/logs/acl "$TREE"/conf.d "$TREE"/out
	chmod 755 "$TREE" "$TREE"/logs
	cd "$TREE" || exit 1

	cat > mail <<-'EOF'
	#!/bin/sh
	{ echo "== mail $*"; cat; echo "== end"; } >> /var/tmp/lrcheck/out/mail.log
	EOF
	chmod 755 mail

	# Logs, with older rotations.
	seq 1 200 > logs/a.log
	echo older1 | gzip > logs/a.log.1.gz
	echo older2 | gzip > logs/a.log.2.gz
	echo older3 | gzip > logs/a.log.3.gz
	chmod 640 logs/a.log logs/a.log.*; chgrp adm logs/a.log
	seq 1 50 > logs/b.log
	echo dated > logs/b.log-20200101
	echo dated2 > logs/b.log-20200102
	seq 1 30 > logs/c.log
	setfacl -m "u:$U:r,g:wheel:r" logs/c.log
	seq 1 2000 > logs/e.log
	echo small > logs/f.log
	: > logs/empty.log
	echo secret > logs/s.log; echo oldsecret > logs/s.log.1
	echo renamed > logs/r.log
	seq 1 5 > logs/x.log
	echo aged > logs/x.log.1; old '40 days ago' logs/x.log.1
	echo user > logs/user/d.log
	chown -R "$U:$U" logs/user; chmod 644 logs/user/d.log
	for n in 1 2 3; do echo "glob $n" > "logs/glob/g$n.log"; done
	echo acl > logs/acl/h.log
	setfacl -m "d:u:$U:rw" logs/acl
	echo included > logs/inc.log
	echo mailed > logs/m.log; echo prev > logs/m.log.1

	cat > lr.conf <<-EOF
	# logrotate-rs rehearsal
	compress
	tabooext + .bak
	include $TREE/conf.d

	$TREE/logs/a.log {
	    rotate 3
	    create 0640 root adm
	    daily
	}
	$TREE/logs/b.log {
	    dateext
	    rotate 1
	    weekly
	    delaycompress
	}
	$TREE/logs/c.log {
	    copytruncate
	    rotate 2
	    monthly
	}
	$TREE/logs/e.log {
	    size 1k
	    compresscmd /usr/bin/xz
	    uncompresscmd /usr/bin/unxz
	    compressext .xz
	}
	$TREE/logs/f.log $TREE/logs/empty.log $TREE/logs/none.log {
	    missingok
	    notifempty
	    daily
	    nocompress
	}
	$TREE/logs/s.log {
	    shred
	    rotate 1
	    daily
	}
	$TREE/logs/r.log {
	    renamecopy
	    olddir $TREE/old
	    createolddir 0750 root adm
	    daily
	}
	$TREE/logs/x.log {
	    maxage 30
	    rotate 5
	    daily
	    nocompress
	}
	$TREE/logs/user/d.log {
	    su $U $U
	    create
	    daily
	    postrotate
	        id -u > $TREE/logs/user/post.uid
	    endscript
	}
	$TREE/logs/glob/*.log {
	    sharedscripts
	    daily
	    firstaction
	        echo "first \$*" >> $TREE/out/scripts.log
	    endscript
	    prerotate
	        echo "pre \$*" >> $TREE/out/scripts.log
	    endscript
	    postrotate
	        echo "post \$*" >> $TREE/out/scripts.log
	    endscript
	    lastaction
	        echo "last \$*" >> $TREE/out/scripts.log
	    endscript
	}
	$TREE/logs/acl/h.log {
	    create
	    daily
	}
	$TREE/logs/m.log {
	    rotate 1
	    mail root@localhost
	    maillast
	    daily
	    nocompress
	}
	EOF
	cat > conf.d/extra <<-EOF
	$TREE/logs/inc.log {
	    rotate 1
	    daily
	}
	EOF
	echo "bad conf, skipped as hidden" > conf.d/.hidden
	echo "bad conf, skipped as taboo" > conf.d/extra.bak

	# Last rotated: a.log yesterday, b.log a week ago, c.log a month ago,
	# the user log long ago; the others have no entry.
	{
		echo "logrotate state -- version 2"
		echo "\"$TREE/logs/a.log\" $(date -d yesterday +%Y-%-m-%-d-%-H:0:0)"
		echo "\"$TREE/logs/b.log\" $(date -d '8 days ago' +%Y-%-m-%-d-%-H:0:0)"
		echo "\"$TREE/logs/c.log\" $(date -d '40 days ago' +%Y-%-m-%-d-%-H:0:0)"
		echo "\"$TREE/logs/user/d.log\" 2020-1-1-0:0:0"
		echo "\"$TREE/logs/m.log\" 2020-1-1-0:0:0"
		echo "\"$TREE/logs/inc.log\" 2020-1-1-0:0:0"
		for n in 1 2 3; do echo "\"$TREE/logs/glob/g$n.log\" 2020-1-1-0:0:0"; done
	} > state
	chmod 640 state

	for f in $(find "$TREE" -type f); do old '2 days ago' "$f"; done
	old '40 days ago' logs/x.log.1
	cd / || exit 1
	tar --acls --xattrs -cpf "$SAVE" -C / "${TREE#/}"
	cp -p /var/lib/misc/logrotate.status "$SAVE.status" 2>/dev/null || rm -f "$SAVE.status"
	echo "saved $TREE in $SAVE"
}

# Dates and pids that differ between runs.
norm() {
	sed -E -e 's/[0-9]{4}-[0-9]{1,2}-[0-9]{1,2}-[0-9]{1,2}:[0-9]{1,2}:[0-9]{1,2}/@DATE@/g' \
		-e 's/\(pid [0-9]+\)/(pid @PID@)/g' \
		-e 's/[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}/@NOW@/g'
}

run() {
	echo "### $*"
	"$@" > /tmp/lrcheck.out 2> /tmp/lrcheck.err
	echo "status $?"
	echo "-- stdout"; norm < /tmp/lrcheck.out
	echo "-- stderr"; norm < /tmp/lrcheck.err
}

tree() {
	echo "### tree"
	find "$TREE" -mindepth 1 | LC_ALL=C sort | while read -r f; do
		printf '%s %s' "${f#$TREE/}" "$(stat -c '%F %a %U:%G' "$f")"
		if [ -f "$f" ]; then
			if [ "$f" -nt "$SAVE" ]; then printf ' new'; else printf ' old'; fi
		fi
		if [ ! -L "$f" ]; then
			getfacl -c -n --skip-base "$f" 2>/dev/null | grep -v '^$' | tr '\n' ' ' | sed 's/^/ /'
		fi
		echo
		if [ -f "$f" ]; then
			case "$f" in
				*.gz) gzip -dc < "$f" ;;
				*.xz) xz -dc < "$f" ;;
				*) cat "$f" ;;
			esac | head -c 300 | norm | sed 's/^/    /'
		fi
	done
}

capture() {
	label=$1
	[ -f "$SAVE" ] || { echo "run setup first" >&2; exit 1; }
	mkdir -p "$RESULTS"
	rm -rf "$TREE"
	tar --acls --xattrs -xpf "$SAVE" -C /
	if [ -f "$SAVE.status" ]; then
		cp -p "$SAVE.status" /var/lib/misc/logrotate.status
	else
		rm -f /var/lib/misc/logrotate.status
	fi
	L="/usr/bin/logrotate"
	{
		if grep -aq 'rust_\|RUST_BACKTRACE' "$L"; then kind=Rust; else kind=C; fi
		echo "# logrotate binary: $kind"
		run "$L" --version
		run "$L" --usage
		run "$L" --help
		run "$L" --bogus
		run "$L" -v -s "$TREE/state" -m "$TREE/mail" "$TREE/lr.conf"
		echo "### state after run 1"; norm < "$TREE/state"
		run "$L" -v -f -s "$TREE/state" -m "$TREE/mail" "$TREE/lr.conf"
		tree
		run "$L" -d -f /etc/logrotate.conf
		run /etc/cron.daily/logrotate
		echo "### system state"; norm < /var/lib/misc/logrotate.status
	} > "$RESULTS/$label.txt" 2>&1
	# The first line names the binary; the rest should match between builds.
	echo "wrote $RESULTS/$label.txt ($(wc -l < "$RESULTS/$label.txt") lines)"
}

case "${1:-}" in
	setup) setup ;;
	capture) [ $# = 2 ] || { echo "usage: $0 capture LABEL" >&2; exit 2; }; capture "$2" ;;
	compare) [ $# = 3 ] || { echo "usage: $0 compare A B" >&2; exit 2; }
		diff -u "$RESULTS/$2.txt" "$RESULTS/$3.txt" && echo "same" ;;
	*) echo "usage: $0 setup | capture LABEL | compare A B" >&2; exit 2 ;;
esac
