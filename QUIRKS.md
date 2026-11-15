# Upstream quirks, kept on purpose

logrotate-rs reproduces logrotate **3.22.0**, as Gentoo builds it, exactly, including
behavior that looks like a bug. Each item here is marked `QUIRK` in the source. Fixing
one is a deliberate, separate change (ideally upstream as well), not something to slip
into the port.

Severity: **high** means it can lose or expose log data, touch the wrong file, or leave
logs unrotated; **medium** means it's surprising and user-visible; **low** means it's
cosmetic or hard to hit.

## Configuration

| Severity | Quirk | Where | Test |
|---|---|---|---|
| **high** | After an error inside a log definition, the parser skips to the closing `}`, but only if the `}` is the first character of its line. An indented `}` is skipped along with its line, so the following definitions (and global directives) are silently dropped, up to the next `}` at the start of a line. | `config.rs` (`STATE_SKIP_CONFIG`) | `braces` (`error_then_indented_close`) |
| medium | A bad `minage` or `maxage` value is reported with the rest of the configuration file, starting from the value's last character, instead of the value: the message prints a pointer into the mapped file as `%s`. | `config.rs` (`minage`/`maxage`) | `counts` |
| medium | Likewise, a bad `mail` address is reported with the rest of the file, starting just after the `mail` keyword. | `config.rs` (`read_address`) | `mail_addresses` |
| **high** | If an `include` inside a log definition fails, the definition is restored from a backup that has no pattern and no files: `-v` shows `rotating pattern: (null)` and the definition's logs are not rotated. When the included path is a single file, the backup was taken before the file was read, so it also restores the definition's old queue link: every definition queued after it (those the included file added, and all later ones in the outer files) silently drops out of the run, while "Handling N logs" still counts them. | `config.rs` (`read_config_path`, `copy_from`) | `includes` (`include_in_definition_error`, `include_in_definition_with_logs_error`) |
| medium | `addextension` set outside a definition never reaches the definitions: `copyLogInfo()` doesn't copy it. | `config.rs` (`LogInfo::copy_from`) | (phase 2) |
| low | A directive with an argument but none given (`size`, `rotate`, `tabooext`...) at the very end of a line skips the line counter, so every later message has the wrong line number. With a trailing blank it doesn't. | `config.rs` (`size`, `tabooext`) | `size_without_value` |
| low | `create`, `su` and `weekly` at the end of a file without a final newline keep their trailing blanks, which can read as an empty extra argument (`create 0640 alice ` fails with "unknown group ''"). | `config.rs` (`isolate_line`) | `create_arguments`, `weekly_arguments` |
| low | A keyword followed by a byte at or above 0x80 is reported as `found 0xffffffc3`: the byte is a signed `char` printed with `%#x`. | `config.rs` (`fmt_hash_x`) | `unknown_and_malformed` |
| low | The `errors` directive warns that it's obsolete but doesn't skip its argument, which is then parsed as a keyword (`errors root@localhost` also fails with "keyword 'root' not properly separated"). | `config.rs` (`errors`) | `errors_directive` |
| low | A FIFO in an included directory, or named on the command line, blocks logrotate forever in `open()`, before the "not a regular file" check. | `config.rs` (`read_config_file`) | none (it hangs) |

## Command line

| Severity | Quirk | Where | Test |
|---|---|---|---|
| low | A bad option is reported as `logrotate: bad argument --foo: unknown error`: the error code given to `poptStrerror()` is `main()`'s `rc`, which is 0. | `main.rs` | `bad_option` |

## Messages

| Severity | Quirk | Where | Test |
|---|---|---|---|
| low | `MESS_REALDEBUG` messages would get the `error: ` prefix (`log_once()` has no case for them). None are sent in 3.22.0. | `log.rs` | none |
