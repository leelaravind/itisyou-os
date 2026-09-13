# Full local test suite: host unit tests + QEMU boot/selftest matrix.
. $PSScriptRoot\env.ps1
Set-Location (Split-Path $PSScriptRoot -Parent)

Write-Output '=== host unit tests ==='
cargo test -p kernel-core -p image-builder -p qemu-runner
if ($LASTEXITCODE -ne 0) { exit 1 }

Write-Output '=== build images ==='
cargo run -p image-builder -- target/images
if ($LASTEXITCODE -ne 0) { exit 1 }

$runner = { param($runnerArgs)
    cargo run -q -p qemu-runner -- @runnerArgs
    if ($LASTEXITCODE -ne 0) { $script:anyFailed = $true }
}
$anyFailed = $false

Write-Output '=== QEMU boot smoke (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
    '--exit-after-markers',
    '--timeout-secs', '120', '--label', 'boot-smoke-bios')

Write-Output '=== QEMU boot smoke (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-uefi.img', '--uefi',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030',
        '--exit-after-markers',
        '--timeout-secs', '120', '--label', 'boot-smoke-uefi')
} else {
    Write-Output 'SKIPPED: UEFI image absent - bootloader UEFI stage blocked upstream (rust-osdev/bootloader#579)'
}

Write-Output '=== QEMU selftest (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-selftest-bios.img', '--nvme',
    '--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B140',
    '--expect', 'B160', '--expect-selftest',
    '--require', 'RING3-HELLO', '--require', 'RING3-DONE',
    '--require', 'user_exit pid=', '--require', 'contained=true',
    '--require', 'RING3-PARENT-DONE', '--require', 'RING3-PARENT-WAIT-OK',
    '--require', 'RING3-PARENT-IPC-OK', '--require', 'RING3-CHILD-EXIT',
    '--require', 'NVMe controller', '--require', 'nvme_ready blocks=',
    '--require', 'SPIN-FINITE-OK',
    '--require', 'gfx width=', '--require', 'RING3-GUI-OK',
    '--expect', 'B170', '--expect', 'B180',
    '--timeout-secs', '300', '--label', 'selftest-bios')

Write-Output '=== QEMU selftest (UEFI) ==='
if (Test-Path 'target/images/itisyou-kernel-selftest-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-selftest-uefi.img', '--uefi', '--nvme',
        '--expect', 'B010', '--expect', 'B020', '--expect', 'B030', '--expect', 'B140',
        '--expect', 'B160', '--expect-selftest',
        '--require', 'RING3-HELLO', '--require', 'RING3-DONE',
        '--require', 'RING3-PARENT-DONE', '--require', 'RING3-PARENT-IPC-OK',
        '--require', 'nvme_ready blocks=', '--require', 'SPIN-FINITE-OK',
        '--timeout-secs', '300', '--label', 'selftest-uefi')
} else {
    Write-Output 'SKIPPED: UEFI image absent - bootloader UEFI stage blocked upstream (rust-osdev/bootloader#579)'
}

Write-Output '=== QEMU shell interaction (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B040', '--expect', 'B050', '--expect', 'B060', '--expect', 'B070',
    '--expect', 'B080', '--expect', 'B090', '--expect', 'B100', '--expect', 'B110',
    '--expect', 'B120', '--expect', 'B130', '--expect', 'B140', '--expect', 'B150',
    '--expect', 'B160',
    '--send', 'help', '--send', 'version', '--send', 'system', '--send', 'memory',
    '--send', 'tasks', '--send', 'uptime', '--send', 'ls /', '--send', 'cat /etc/version',
    '--send', 'echo shell-echo-check', '--send', 'definitely-not-a-command',
    '--send', 'run /bin/init', '--send', 'run /bin/broken',
    '--send', 'panic-test', '--send', 'shutdown',
    # The running kernel must name the version the release claims
    # (Cargo workspace version == status/current.json; check-consistency.mjs).
    '--require', 'itisyou-os 0.8.1',
    '--require', 'task 0: kmain',
    '--require', 'RING3-DONE',
    '--require', 'run: /bin/init: Exit(0)',
    '--require', 'load failed: Elf(BadMagic)',
    '--require', 'physical: usable_frames=',
    '--require', 'heap: used=',
    '--require', 'etc/',
    '--require', 'shell-echo-check',
    '--require', 'unknown command: definitely-not-a-command',
    '--require', "panic-test: pass 'confirm'",
    '--require', 'shutting down (QEMU exit)',
    '--timeout-secs', '90', '--label', 'shell-test-bios')

Write-Output '=== QEMU desktop graphics + PS/2 input (BIOS) ==='
# Boot to the shell, enter the graphical desktop, then inject real keyboard and
# mouse events through the QEMU HMP monitor (into the emulated PS/2 devices).
# The kernel's IRQ1/IRQ12 handlers must observe them (INPUT markers) and the
# compositor captures a framebuffer screendump - the OS renders the desktop,
# no host UI. DESKTOP-INPUT-VERIFIED proves the full graphics+input path.
$deskShot = Join-Path (Resolve-Path 'artifacts/qemu') 'desktop.ppm'
Remove-Item $deskShot -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B170', '--expect', 'B180',
    '--send', 'desktop',
    '--inject-after', 'DESKTOP-READY',
    '--monitor-cmd', 'sendkey h', '--monitor-cmd', 'sendkey e', '--monitor-cmd', 'sendkey l',
    '--monitor-cmd', 'sendkey l', '--monitor-cmd', 'sendkey o',
    '--monitor-cmd', 'mouse_move 40 25', '--monitor-cmd', 'mouse_move -20 15',
    '--monitor-cmd', 'mouse_button 1', '--monitor-cmd', 'mouse_button 0',
    '--monitor-cmd', "screendump $deskShot",
    '--monitor-cmd', 'sendkey esc',
    '--require', '[ITISYOU:MODE] desktop',
    '--require', 'DESKTOP-READY',
    '--require', '[ITISYOU:INPUT] key=h',
    '--require', '[ITISYOU:INPUT] key=o',
    '--require', '[ITISYOU:INPUT] mouse dx=',
    '--require', 'DESKTOP-INPUT-VERIFIED',
    '--timeout-secs', '150', '--label', 'desktop-input-bios')

Write-Output '=== QEMU device model + AC97 audio (BIOS) ==='
# Attach an AC97 audio controller whose output is captured to a WAV via QEMU's
# `wav` backend. The OS binds the ac97 driver (B190 device model), the `beep`
# command synthesizes a tone and DMAs it through the PCM-Out bus master, and
# the runner asserts (a) the driver init + full buffer consumption markers and
# (b) that the captured WAV contains non-silent PCM — proving the generated
# samples traversed driver -> controller -> output, not just init.
$audioWav = Join-Path (Resolve-Path 'artifacts/qemu') 'audio.wav'
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190',
    '--audio', '--audio-out', $audioWav,
    '--send', 'lsdev', '--send', 'beep', '--send', 'shutdown',
    '--require', 'devmodel devices=',
    '--require', 'driver_bound name=', '--require', 'ac97 init',
    '--require', 'ac97 play', '--require', 'halted=true',
    '--timeout-secs', '150', '--label', 'audio-bios')

Write-Output '=== QEMU USB (UHCI) enumeration + HID input into the desktop (BIOS) ==='
# Attach a UHCI controller + USB HID keyboard. The OS binds the uhci driver
# (B190), resets the root port, and enumerates over control transfers (device +
# configuration descriptors, SET_ADDRESS/CONFIG/PROTOCOL). Then the graphical
# desktop polls the HID interrupt endpoint through the *unified* input queue
# (same InputEvent stream as PS/2); an injected keypress reaches the desktop
# tagged src=usb - proving both real USB HID input and input-source unification.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190', '--usb',
    '--send', 'desktop',
    '--inject-after', 'DESKTOP-READY',
    '--monitor-cmd', 'sendkey g', '--monitor-cmd', 'sendkey esc',
    '--require', 'uhci ready', '--require', 'usb device vendor=',
    '--require', 'usb hid iface=', '--require', 'key=g src=usb',
    '--require', 'DESKTOP-INPUT-VERIFIED',
    '--timeout-secs', '150', '--label', 'usb-hid-bios')

Write-Output '=== QEMU xHCI enumeration + HID input (BIOS) ==='
# A second, structurally different USB host controller. UHCI has the driver
# build transfer descriptors into a frame list the controller walks; xHCI is
# ring-based and command-driven - the driver posts TRBs, rings a doorbell, and
# reads every result off an event ring the controller owns. Sharing the
# descriptor parsing in kernel-core and nothing else is the point: the same
# `usb` module now serves two genuinely different controllers.
#
# The leg proves the whole chain rather than just binding: controller reset,
# command and event rings, Enable Slot, Address Device, two control IN
# transfers (device then configuration descriptor), Configure Endpoint,
# SET_CONFIGURATION and SET_PROTOCOL on the device, and finally an interrupt
# transfer carrying a real injected keypress - tagged `src=xhci` so it can
# never be confused with the UHCI leg's input.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--xhci',
    '--expect', 'B190', '--expect', 'B210',
    '--send', 'lsdev',
    '--send', 'xhciwait',
    '--inject-after', 'XHCI-HID-WAITING',
    '--monitor-cmd', 'sendkey g',
    '--send', 'shutdown',
    '--require', 'xhci ready caplength=',
    '--require', 'driver: xhci',
    '--require', 'xhci device vendor=0x0627',
    '--require', 'xhci hid iface=0 endpoint=0x81',
    '--require', 'XHCI-HID-REPORT bytes=8',
    '--require', 'ascii=g',
    '--require', '[ITISYOU:INPUT] key=g src=xhci',
    '--forbid', 'xhci enumerate_failed',
    '--timeout-secs', '240', '--label', 'xhci-hid-bios')

Write-Output '=== QEMU system platform: caps, sandbox, services, packages (BIOS) ==='
# The V0.7 platform driven through the shell over a persistent NVMe store:
# a zero-capability probe proves default deny; a sandboxed fs-probe proves the
# path sandbox; the service supervisor runs (echod serves a client, crashd is
# restarted 3x then Failed); packages install/launch/update/rollback with the
# hello-app running under manifest-only capabilities; the audit trail shows
# denials + privileged actions. All values are real kernel/platform state.
$platDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-platform-test.img'
Remove-Item $platDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $platDisk,
    '--expect', 'B190',
    '--send', 'run /bin/sandbox-probe -',
    '--send', 'run /bin/cap-handle-probe',
    '--send', 'run /bin/fs-probe fs_read /etc',
    '--send', 'svc',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'pkg install /pkgs/hello-app-bad.itpkg',
    # V0.8 authenticity fixtures. Each is INTACT — correct digest, valid
    # manifest — so only the trust check can refuse them, and each fails
    # for its own reason.
    '--send', 'pkg install /pkgs/hello-app-unsigned.itpkg',
    '--send', 'pkg install /pkgs/hello-app-untrusted.itpkg',
    '--send', 'pkg install /pkgs/hello-app-forged.itpkg',
    '--send', 'pkg launch hello-app',
    '--send', 'pkg install /pkgs/hello-app-2.itpkg',
    '--send', 'pkg rollback hello-app',
    '--send', 'pkg launch hello-app',
    '--send', 'pkg list',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'SANDBOX-DENIED-OK',
    '--require', 'FS-SANDBOX-OK',
    # V0.8 capability-handle ENFORCEMENT (not just issuance): a forged
    # generation, an expired handle and a revoked handle are each refused on
    # the real syscall path, with the precise reason in the audit trail.
    '--require', 'CAPH-FORGED-DENIED',
    '--require', 'CAPH-EXPIRED-DENIED',
    '--require', 'CAPH-REVOKED-DENIED',
    '--require', 'CAPH-ENFORCEMENT-OK',
    '--require', 'reason=expired',
    '--require', 'reason=no_handle',
    '--require', 'ECHOD-SERVED-3', '--require', 'SVC-CLIENT-OK',
    '--require', 'name=crashd state=failed pid=', '--require', 'restarts=3',
    '--require', 'install name=hello-app v=1 result=ok',
    '--require', 'verify result=refused reason=DigestMismatch',
    # Signed install and launch, then the three authenticity refusals.
    '--require', 'signature result=ok signer=',
    '--require', 'signature result=refused reason=unsigned',
    '--require', 'signature result=refused reason=untrusted_signer',
    '--require', 'signature result=refused reason=bad_signature',
    '--require', 'action=pkg_verify_signature cap=0x0 result=denied',
    '--require', 'action=pkg_launch_signature',
    '--require', 'HELLO-APP-OK',
    '--require', 'rollback name=hello-app from=v2 to=v1 result=ok',
    '--require', 'result=denied',
    '--timeout-secs', '240', '--label', 'platform-bios')

Write-Output '=== QEMU long-running background services (BIOS) ==='
# V0.8 persistent services: `tickd` and `flapd` are ordinary Ring 3 processes
# started at boot with exactly the capabilities they declare (tickd IPC-only,
# flapd none), co-scheduled with the interactive shell instead of run to
# completion. The assertions prove all four properties that distinguish a
# long-running service from V0.7's run-to-completion tasks:
#   * it is still ALIVE later in the boot - heartbeats are stamped with real
#     kernel ticks, not scheduling passes, so the cadence is load-independent;
#   * it is still SERVING, not merely resident - two `bg` clients complete an
#     IPC round trip against the SAME instance (tickd's own counter reaches
#     n=2, which a restarted service could never print);
#   * a daemon's clean exit is a FAULT - flapd returns immediately and is
#     restarted under the bounded policy, then marked Failed at the ceiling
#     instead of restarting forever;
#   * the system stays healthy and interactive throughout - the on-demand
#     supervisor still runs its own services correctly alongside them, the
#     `svc` table reports background and on-demand state together, and the
#     shell still accepts commands afterwards.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B190',
    '--send', 'bg /bin/tick-client',
    '--send', 'bg /bin/spin-finite',
    '--send', 'svc',
    '--send', 'bg /bin/tick-client',
    '--send', 'shutdown',
    '--require', 'bg_start name=tickd pid=1 caps=0x2 long_running=true',
    '--require', 'bg_start name=flapd pid=2 caps=0x0 long_running=true',
    '--require', 'TICKD-READY',
    '--require', 'TICKD-ALIVE tick=',
    '--require', 'TICKD-SERVED n=2',
    '--require', 'TICKC-OK passes=',
    '--require', 'bg_restart name=flapd pid=',
    '--require', 'clean_exit=true',
    '--require', 'bg_failed name=flapd restarts=3',
    '--require', 'name=flapd state=failed',
    '--require', 'tickd  Running',
    '--require', 'flapd  Failed { restarts: 3 }',
    '--require', 'SVC-CLIENT-OK',
    '--require', 'svc: started=3 done=2 failed=1 restarts=3',
    '--require', 'shutting down (QEMU exit)',
    '--timeout-secs', '240', '--label', 'services-bg-bios')

Write-Output '=== QEMU networking: e1000 + ARP/IPv4/ICMP/UDP/DNS (BIOS) ==='
# The guest's NIC is wired to the runner's own host-side Ethernet peer over a
# `dgram` netdev, so the runner IS the network: no slirp, no host resolver,
# nothing outside this machine. The peer is an independent implementation
# (tools/qemu-runner/src/wire.rs), so a bug in the guest's codec cannot cancel
# itself out against the same code on the other side.
#
# The leg proves the data path in BOTH directions and the capability gate:
#   * client paths - ARP resolve, ICMP echo, a Ring 3 app completing a UDP
#     round trip through the socket ABI, and a DNS A lookup;
#   * responder paths - the guest answering the peer's ARP request and ping,
#     which its own traffic never exercises;
#   * refusal - five hostile frames (corrupt IP checksum, ping addressed
#     elsewhere, UDP to an unbound port, a VLAN tag, an ARP with a
#     contradictory hardware type) counted as refused by the guest and
#     answered by NOTHING (`replies_to_hostile=0`);
#   * authority - an app holding `network` completes the round trip; an app
#     with no capabilities is denied every network syscall.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net',
    '--expect', 'B190', '--expect', 'B200',
    '--send', 'run /bin/net-probe network',
    '--send', 'run /bin/net-denied -',
    '--send', 'ping 10.0.2.2 2',
    '--send', 'resolve os.itisyou.app',
    '--send', 'net',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'nic_ready driver=e1000 mac=52:54:00:12:34:56 link_up=true',
    '--require', 'iface_up ip=10.0.2.15 gateway=10.0.2.2',
    # Ring 3 client path, end to end.
    '--require', 'NETPROBE-IFACE mac=52:54:00:12:34:56 ip=10.0.2.15 link_up=1',
    '--require', 'NETPROBE-BOUND port=40100',
    '--require', 'NETPROBE-ECHO-OK bytes=25 from=10.0.2.2',
    '--require', 'NETPROBE-RESOLVE-OK name=os.itisyou.app address=93.184.216.34',
    '--require', 'NETPROBE-OK',
    # Capability gate: every network syscall denied without a handle.
    '--require', 'NETDENY-OK call=net_info',
    '--require', 'NETDENY-OK call=udp_bind',
    '--require', 'NETDENY-OK call=udp_send',
    '--require', 'NETDENY-OK call=udp_recv',
    '--require', 'NETDENY-OK call=net_resolve',
    '--require', 'NETDENY-ALL-DENIED',
    '--require', 'action=udp_bind cap=0x100 result=ok',
    # And the denial is audited with a precise reason, not a bare refusal.
    '--require', 'action=udp_bind cap=0x0 result=denied',
    '--require', 'action=net_info cap=0x0 result=denied',
    # The reason is recorded too, so a refusal is diagnosable rather than a
    # bare ERR_PERM. (Asserted separately: an embedded quote in a require
    # string does not survive native-argument quoting on Windows.)
    '--require', 'kind=network reason=no_handle',
    # Kernel client paths.
    '--require', 'PING-SUMMARY target=10.0.2.2 sent=2 received=2',
    '--require', 'RESOLVE-OK name=os.itisyou.app address=93.184.216.34',
    # What the HOST observed: the guest really put these frames on the wire,
    # with checksums that verify against an independent implementation.
    '--require', '[HOST:NET] guest_mac=52:54:00:12:34:56',
    '--require', '[HOST:NET] guest_ip=10.0.2.15',
    '--require', '[HOST:NET] guest_arp_reply from=10.0.2.15',
    '--require', '[HOST:NET] guest_icmp_reply id=4919',
    '--require', '[HOST:NET] dns_query name=os.itisyou.app type=1 class=1',
    '--require', '[HOST:NET] hostile_probe_start',
    '--require', 'guest_arp_replies=1 guest_icmp_replies=1 hostile_sent=true replies_to_hostile=0',
    '--require', 'bad_ip_csum=0 bad_udp_csum=0 bad_icmp_csum=0',
    # The guest counted every hostile frame as refused, by the right reason.
    '--require', 'rx_malformed=3 rx_unwanted=2',
    '--timeout-secs', '240', '--label', 'net-bios')

Write-Output '=== QEMU hardening: SMEP/SMAP/UMIP, W^X, stack guard (BIOS) ==='
# Every leg in this matrix already runs on a CPU that advertises SMEP, SMAP and
# UMIP (see the runner's -cpu line), so the whole suite passing is itself
# evidence that supervisor-mode protection did not break the kernel's own
# legitimate access to user memory. This leg asserts the protections are ON and
# that they REFUSE things.
#
# Two assertions are about absence, which is what `--forbid` is for: "the CPU
# refused" prints no line of its own, so the only way to state it is that the
# marker printed on success never appeared.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'harden',
    '--send', 'run /bin/init',
    '--send', 'run /bin/wx-test',
    '--send', 'run /bin/harden-probe',
    '--send', 'run /bin/stack-guard-probe',
    '--send', 'run /bin/pf-test',
    '--send', 'shutdown',
    '--require', 'cpu_protection smep=true smap=true umip=true',
    '--require', 'harden: smep=true smap=true umip=true',
    # SMAP is on and the kernel still reads and writes user buffers correctly
    # through its three declared windows - RING3-DONE comes from a `write`.
    '--require', 'RING3-DONE',
    # W^X: a segment marked both writable and executable is refused at load.
    '--require', 'load failed: WxSegment',
    # UMIP: `sgdt` from Ring 3 is a #GP, contained.
    '--require', 'HARDEN-PROBE-SGDT',
    '--require', 'vector=13 addr=0x0 contained=true',
    # The stack is exactly as large as the kernel says, and the page below it
    # is not writable.
    '--require', 'STACKGUARD-WROTE page=15',
    '--require', 'vector=14 addr=0x7ffffdf000 contained=true',
    # A Ring 3 read of a kernel address still faults and is contained.
    '--require', 'addr=0xffff8000dead0000 contained=true',
    # Absence assertions: neither probe may ever reach its success path.
    '--forbid', 'HARDEN-LEAK-SGDT',
    '--forbid', 'STACKGUARD-LEAK',
    '--timeout-secs', '240', '--label', 'harden-bios')

Write-Output '=== QEMU interrupt modernization: APIC + I/O APIC + MSI-X (BIOS) ==='
# The local APIC comes up ALONGSIDE the legacy PIC, which keeps serving the
# timer and PS/2 input; the leg asserts both, so "the APIC works" can never be
# read as "the verified V0.7 path was replaced".
#
# Two of the three claims are delivery, not configuration:
#   * a one-shot local-APIC timer interrupt arrives on vector 0x41;
#   * a real NVMe block read's completion raises MSI-X on vector 0x42.
# The third, the I/O APIC, is programmed and read back but left MASKED - line
# IRQs stay on the PIC. The assertion says `masked=true` so the evidence
# cannot be mistaken for line-based delivery through the I/O APIC.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net', '--nvme',
    '--expect', 'B190', '--expect', 'B200', '--expect', 'B210',
    '--send', 'irq',
    '--send', 'shutdown',
    '--require', 'lapic_ready id=0',
    '--require', 'ioapic id=0',
    '--require', 'irq=1 vector=0x43 readback=ok masked=true',
    '--require', 'legacy PIC path, still primary',
    '--require', 'irq: apic_timer delivered=true count=1 vector=0x41',
    '--require', 'msix_enabled dev=00:04.0 vector=0x42',
    '--require', 'irq: msix armed=true block_read=true delivered=true',
    '--require', 'spurious=0',
    '--timeout-secs', '240', '--label', 'irq-bios')

Write-Output '=== QEMU userspace filesystem writes (BIOS, two boots) ==='
# V0.8 capability-scoped Ring 3 writes to the persistent ITFS store. Boot 1
# walks the whole contract and boot 2 (a fresh guest, same disk) proves the
# write survived a real reboot through the USERSPACE path - a different claim
# from the V0.4 kernel-side persistence test, because the capability check,
# the sandbox check, the store-name parsing and the atomic commit all sit
# between the program and the disk.
#
# The error paths matter as much as the successes: four refusals, each for a
# different reason, so one over-broad check cannot pass by accident. And
# `fs-write-denied` holds `fs_read` and nothing else, so its refusals prove
# the WRITE right specifically - it can still read and list the same store.
$fsDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-fswrite-test.img'
Remove-Item $fsDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $fsDisk,
    '--expect', 'B190',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'store put planted.txt planted',
    '--send', 'run /bin/fs-write-denied fs_read',
    '--send', 'run /bin/fs-writer fs_read,fs_write /pkgs',
    '--send', 'run /bin/fs-user-persist fs_read,fs_write',
    '--send', 'store ls',
    '--send', 'audit',
    '--send', 'shutdown',
    # Create, atomic overwrite to a different length, list, delete.
    '--require', 'FSWRITE-CREATED bytes=17',
    '--require', 'FSWRITE-OVERWROTE bytes=34',
    '--require', 'FSWRITE-LISTED',
    '--require', 'FSWRITE-ERRORS-OK',
    '--require', 'FSWRITE-DELETED',
    '--require', 'FSWRITE-OK',
    # The write right, specifically: refused for a process holding fs_read.
    '--require', 'FSDENY-OK call=fs_write',
    '--require', 'FSDENY-OK call=fs_delete',
    '--require', 'FSDENY-READ-STILL-ALLOWED',
    '--require', 'FSDENY-LIST-STILL-ALLOWED',
    '--require', 'FSDENY-ALL-DENIED',
    # The sandbox is separate from the capability: the same program, holding
    # fs_write, confined to /pkgs, cannot write to /data.
    '--require', 'FSWRITE-FAILED step=create',
    '--require', 'action=fs_write_sandbox cap=0x80 result=denied',
    # Privileged mutations are audited with the path and size.
    '--require', 'action=fs_write cap=0x80 result=ok',
    '--require', 'action=fs_delete cap=0x80 result=ok',
    '--require', 'FSUSER-WROTE bytes=38',
    '--timeout-secs', '240', '--label', 'fs-write-bios')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $fsDisk,
    '--expect', 'B190',
    '--send', 'run /bin/fs-user-persist fs_read,fs_write',
    '--send', 'store ls',
    '--send', 'shutdown',
    # A fresh guest on the same disk reads back exactly what Ring 3 wrote.
    '--require', 'FSUSER-VERIFIED bytes=38',
    '--timeout-secs', '240', '--label', 'fs-write-persist')
Remove-Item $fsDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU persistent audit trail (BIOS, three boots) ==='
# Provenance has to outlive the process that produced it, and it has to be
# possible to tell an edited trail from an intact one. Records are chained -
# each hash covers the previous one - so altering, deleting, reordering or
# inserting a record changes the head.
#
# Boot 1 does privileged work and persists the trail. Boot 2 is a FRESH guest
# on the same disk: it recovers the trail, recomputes the chain and reports
# `verified` with the same head boot 1 wrote. Boot 3 overwrites the stored
# trail with a forged header and must report `TAMPERED` - the negative case
# matters more than the positive one, because a verifier that never fails is
# not a verifier.
$auditDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-audit-test.img'
Remove-Item $auditDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $auditDisk,
    '--expect', 'B210',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'audit',
    '--send', 'audit save',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=absent',
    '--require', 'audit: chain boot=0',
    '--require', 'trail_saved records=',
    '--timeout-secs', '240', '--label', 'audit-persist-write')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $auditDisk,
    '--expect', 'B210',
    '--send', 'audit',
    '--send', 'shutdown',
    # A fresh guest recomputes the chain over the recovered records and gets
    # the head the previous boot wrote.
    '--require', 'trail_recovered status=verified records=',
    '--require', 'audit: chain boot=1',
    '--timeout-secs', '240', '--label', 'audit-persist-verify')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $auditDisk,
    '--expect', 'B210',
    '--send', 'store put audit.log itisyou-audit v1 boot=0 count=1 head=0000000000000000000000000000000000000000000000000000000000000000',
    '--send', 'audit verify',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=verified',
    '--require', 'trail_recovered status=TAMPERED',
    '--timeout-secs', '240', '--label', 'audit-persist-tamper')
Remove-Item $auditDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU interrupted update -> recovery (BIOS, two boots) ==='
# Boot 1 installs v1 and STAGES v2 without committing (a simulated crash mid-
# update), then powers off. Boot 2 (fresh guest, same disk) must find v1
# still active, detect the orphaned staged v2, remove it via the recovery
# scan, and launch v1 - an interrupted update can never activate, and
# recovery works across a real reboot.
$updDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-update-test.img'
Remove-Item $updDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $updDisk,
    '--expect', 'B190',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'pkg stage /pkgs/hello-app-2.itpkg',
    '--send', 'shutdown',
    '--require', 'install name=hello-app v=1 result=ok',
    '--require', 'stage name=hello-app v=2',
    '--timeout-secs', '180', '--label', 'update-interrupt')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $updDisk,
    '--expect', 'B190',
    '--send', 'pkg recover',
    '--send', 'pkg launch hello-app',
    '--send', 'shutdown',
    '--require', 'orphan_staged=v2 action=remove',
    '--require', 'launch name=hello-app v=1',
    '--require', 'HELLO-APP-OK',
    '--timeout-secs', '180', '--label', 'update-recovery')
Remove-Item $updDisk -ErrorAction SilentlyContinue
Remove-Item $platDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU intentional panic (BIOS) ==='
& $runner @('--image', 'target/images/itisyou-kernel-panictest-bios.img',
    '--expect', 'B010', '--expect', 'B020', '--expect-panic',
    '--require', 'intentional panic-test',
    '--timeout-secs', '60', '--label', 'panic-test-bios')

Write-Output '=== QEMU filesystem reboot persistence (UEFI, two boots) ==='
# Two separate QEMU guests share one disposable persistent disk: boot 1
# formats ITFS + writes /hello, boot 2 (fresh guest) mounts + verifies it —
# proving the write survived a full reboot through the real block layer.
# (UEFI: the fs-persist binary's BIOS image does not boot — see
# docs/DEVELOPMENT_STORY; UEFI is a fully verified firmware path.)
$persistDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-fs-persist-test.img'
Remove-Item $persistDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-fs-persist-uefi.img', '--uefi',
    '--nvme-persist', $persistDisk,
    '--expect', 'B010', '--expect', 'B160',
    '--require', 'FS-PERSIST-WROTE',
    '--timeout-secs', '90', '--label', 'fs-persist-write')
& $runner @('--image', 'target/images/itisyou-fs-persist-uefi.img', '--uefi',
    '--nvme-persist', $persistDisk,
    '--expect', 'B010', '--expect', 'B160',
    '--require', 'FS-PERSIST-VERIFIED',
    '--timeout-secs', '90', '--label', 'fs-persist-verify')
Remove-Item $persistDisk -ErrorAction SilentlyContinue

if ($anyFailed) { Write-Output 'TEST: FAILED'; exit 1 }
Write-Output 'TEST: OK'
exit 0
