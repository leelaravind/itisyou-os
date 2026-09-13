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
    '--require', 'itisyou-os 0.11.0-dev',
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
    # V0.10: the FIRST packet decodes to what was sent (a stray ACK used to
    # frame it off by one), with Y screen-down like every other source.
    '--require', 'mouse dx=40 dy=25 ', '--require', 'mouse dx=-20 dy=15 ',
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
    '--require', 'CAPH-ENFORCEMENT-OK', '--require', 'CAPH-NULL-LIST-OK',
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

Write-Output '=== QEMU signing-key hierarchy: root, certificates, revocation (BIOS) ==='
# V0.9 KEY09-001. The kernel trusts only an offline root (public key compiled
# in); signing keys are trusted through root-signed certificates in
# /etc/trust, each with a scope and a validity window in release epochs, and a
# root-signed revocation list retires keys. Every refused fixture is intact
# and correctly signed — only the chain can refuse it, each for its own
# reason. The published test key signing a package outside its scope is the
# case that makes publishing it safe.
$trustDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-trust-test.img'
Remove-Item $trustDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $trustDisk,
    '--expect', 'B190',
    '--send', 'pkg trust',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'pkg install /pkgs/hello-app-retired.itpkg',
    '--send', 'pkg install /pkgs/hello-app-expired.itpkg',
    '--send', 'pkg install /pkgs/hello-app-rogue.itpkg',
    '--send', 'pkg install /pkgs/other-app-1.itpkg',
    '--send', 'pkg install /pkgs/hello-app-untrusted.itpkg',
    '--send', 'pkg launch hello-app',
    '--send', 'pkg list',
    '--send', 'shutdown',
    '--require', '[ITISYOU:TRUST] root=679e3823 epoch=9',
    '--require', 'cert file=rogue-signer.cert result=rejected reason=bad_root_signature',
    '--require', 'action=trust_load_certificate cap=0x0 result=denied',
    '--require', 'revocations seq=1 count=1 result=ok',
    '--require', 'certs_loaded=4 rejected=1',
    '--require', 'label=retired-signer',
    '--require', 'epochs=9..=12 state=revoked',
    '--require', 'epochs=7..=8 state=expired',
    '--require', 'signature result=ok signer=c645535e key_id=1 label=test-signer',
    '--require', 'install name=hello-app v=1 result=ok',
    '--require', 'signature result=refused reason=revoked_signer',
    '--require', 'signature result=refused reason=certificate_expired',
    '--require', 'signature result=refused reason=out_of_scope',
    '--require', 'signature result=refused reason=untrusted_signer',
    '--require', 'HELLO-APP-OK',
    '--forbid', 'stage name=other-app',
    '--forbid', 'store v2',
    '--timeout-secs', '240', '--label', 'trust-bios')
Remove-Item $trustDisk -ErrorAction SilentlyContinue

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
    '--require', 'bg_start name=tickd pid=2 caps=0x2 long_running=true',
    '--require', 'bg_start name=flapd pid=3 caps=0x0 long_running=true',
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

Write-Output '=== QEMU DHCP against QEMU user-mode networking (BIOS) ==='
# QEMU's DHCP server is an independent implementation, and it is told to serve
# 10.0.9.0/24 - NOT the guest's static 10.0.2.15/24 plan. So the gateway is
# unreachable until the lease is applied (the first ping must fail), and
# 10.0.9.50 can only come from the lease. The reply is a broadcast, which also
# guards the V0.9 fix for broadcast UDP (V0.8 checksummed every broadcast
# against the host's own address and dropped it as malformed).
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user', '--net-user-extra', 'net=10.0.9.0/24,dhcpstart=10.0.9.50',
    '--expect', 'B200',
    '--send', 'ping 10.0.9.2 1',
    '--send', 'dhcp',
    '--send', 'net',
    '--send', 'ping 10.0.9.2 2',
    '--send', 'shutdown',
    '--require', 'PING-SUMMARY target=10.0.9.2 sent=1 received=0',
    '--require', 'dhcp_offer ip=10.0.9.50 server=10.0.9.2',
    '--require', 'dhcp_lease ip=10.0.9.50 mask=255.255.255.0 router=10.0.9.2 dns=10.0.9.3 server=10.0.9.2 lease_secs=86400',
    '--require', 'DHCP-OK ip=10.0.9.50',
    '--require', 'net: mac=52:54:00:12:34:56 ip=10.0.9.50 mask=255.255.255.0 gateway=10.0.9.2',
    '--require', 'PING-SUMMARY target=10.0.9.2 sent=2 received=2',
    '--require', 'action=dhcp_lease cap=0x0 result=ok',
    '--forbid', 'DHCP-FAILED',
    '--timeout-secs', '180', '--label', 'net-dhcp-bios')

Write-Output '=== QEMU IPv6 foundations against QEMU user-mode networking (BIOS) ==='
# QEMU's user-mode network is an independent IPv6 router (fe80::2, prefix
# fec0::/64). The guest derives its link-local address from its MAC, joins the
# all-nodes and solicited-node groups in the NIC's multicast filter, solicits a
# router, forms a SLAAC address from the advertised prefix, resolves the
# router's global address by neighbour solicitation and pings it - while IPv4
# keeps working alongside.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user',
    '--expect', 'B200',
    '--send', 'ipv6',
    '--send', 'ping6 fec0::2 2',
    '--send', 'ping 10.0.2.2 1',
    '--send', 'net',
    '--send', 'shutdown',
    '--require', '[ITISYOU:NET6] up link_local=fe80::5054:ff:fe12:3456',
    '--require', '[ITISYOU:NET6] slaac global=fec0::5054:ff:fe12:3456 router=fe80::2',
    '--require', 'IPV6-OK global=fec0::5054:ff:fe12:3456',
    '--require', 'PING6-SUMMARY target=fec0::2 sent=2 received=2',
    '--require', 'PING-SUMMARY target=10.0.2.2 sent=1 received=1',
    '--require', 'net6: link_local=fe80::5054:ff:fe12:3456 global=fec0::5054:ff:fe12:3456 router=fe80::2',
    '--require', 'rx_malformed=0 rx_unwanted=0 router_adverts=1',
    '--forbid', 'IPV6-NO-ROUTER',
    '--timeout-secs', '180', '--label', 'net-ipv6-bios')

Write-Output '=== QEMU IPv6 responder paths against the harness peer (BIOS) ==='
# QEMU's router never solicits the guest, so the guest's RESPONDER paths need
# a peer that does: the harness's own byte-level implementation solicits the
# guest's link-local address (to its solicited-node group) and pings it,
# checking every ICMPv6 checksum itself. With no router on this wire the guest
# must stay link-local only.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net',
    '--expect', 'B200',
    '--send', 'ipv6',
    # The peer solicits and pings the guest once it has seen it; poll for a
    # fixed window so the answers do not depend on when those frames land.
    '--send', 'net poll 3000',
    '--send', 'net',
    '--send', 'shutdown',
    '--require', 'IPV6-NO-ROUTER (link-local only)',
    '--require', 'net: polled ms=3000',
    '--require', '[HOST:NET6] guest_na target=fe80:0:0:0:5054:ff:fe12:3456 solicited=true override=true',
    '--require', '[HOST:NET6] guest_echo6_reply from=fe80:0:0:0:5054:ff:fe12:3456 payload_ok=true',
    '--require', 'neighbor_adverts_sent=1 echo_replies_sent=1',
    '--require', 'guest_na=1 guest_icmp6_replies=1 bad_icmp6_csum=0',
    '--timeout-secs', '180', '--label', 'net-ipv6-responder-bios')

Write-Output '=== QEMU TCP stream against the host OS TCP stack (BIOS) ==='
# QEMU's user-mode network maps the guest's 10.0.2.2 to the host's loopback,
# so the peer is the host operating system's own TCP implementation, sharing
# no code with the guest's. The harness's echo endpoint checks the byte
# pattern itself. Three runs: a clean stream; the same stream with the next
# two outgoing data segments discarded, recovered by the retransmission
# timer; and the probe without the network capability, refused and audited.
# First, the PASSIVE open: the console listens on port 7 and a host client
# connects through a slirp port forward, sends the pattern and checks the
# echo itself.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user', '--tcp-echo-port', '47123', '--tcp-client-forward', '7',
    '--expect', 'B200',
    '--send', 'tcp serve 7',
    '--send', 'run /bin/tcp-probe network',
    '--send', 'tcp drop 2',
    '--send', 'run /bin/tcp-probe network',
    '--send', 'run /bin/tcp-probe -',
    '--send', 'run /bin/tcp-server-probe network',
    '--send', 'run /bin/tcp-server-probe -',
    '--send', 'tcp',
    '--send', 'shutdown',
    '--require', 'action=tcp_connect cap=0x100 result=ok',
    '--require', 'TCPPROBE-CONNECTED',
    '--require', 'TCPPROBE-NULL-SEND-OK',
    '--require', 'TCPPROBE-ECHO-OK bytes=3000',
    '--require', 'TCPPROBE-CLOSED',
    '--require', 'TCPPROBE-OK',
    '--require', 'TCPSERVE-LISTENING port=7',
    '--require', 'TCPSERVE-ACCEPTED from=10.0.2.2:',
    '--require', 'TCPSERVE-ECHOED bytes=2500',
    '--require', 'TCPSERVE-CLOSED echoed=2500',
    '--require', '[HOST:TCPC] echo_ok',
    '--require', 'TCPSERVE-LISTENING port=7 ring=3',
    '--require', 'action=tcp_listen cap=0x100 result=ok',
    '--require', 'TCPSERVER-ACCEPTED',
    '--require', 'TCPSERVER-ECHOED bytes=2500',
    '--require', 'TCPSERVER-OK',
    '--require', 'TCPSERVER-DENIED call=tcp_listen',
    '--require', 'exchanges=2 attempts=2 echo_ok=true bytes=5000',
    '--forbid', 'TCPSERVER-FAILED',
    '--forbid', 'owner_state=dead',
    '--require', '[ITISYOU:TCP] injected_loss seq=',
    '--require', '[ITISYOU:TCP] retransmit seq=',
    '--require', 'injected_losses=2',
    '--require', 'malformed=0',
    '--require', 'TCPPROBE-DENIED call=tcp_connect',
    '--require', 'action=tcp_connect cap=0x0 result=denied',
    '--require', '[HOST:TCP] summary connections=2 bytes_in=6000 bytes_out=6000 pattern_ok=true clean_close=true',
    '--forbid', 'TCPPROBE-FAILED',
    '--timeout-secs', '240', '--label', 'net-tcp-bios')

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

Write-Output '=== QEMU program arguments: console and parent-to-child (BIOS) ==='
# V0.10 PROC10-001. Every process carries an immutable argument block (at most
# 16 arguments, 512 bytes, printable ASCII without spaces), validated once
# when the process is created and read back with the capability-free `args`
# syscall. The leg proves both launch paths and the refusals:
#   * console: `run <path> - - -- alpha beta gamma` arrives in order, and the
#     syscall's own contract holds (a buffer one byte short is ERR_2BIG with
#     nothing written; a kernel pointer is ERR_FAULT);
#   * console refusal: 17 arguments are refused by the validator with a clear
#     message and the program never runs (`value=q` is forbidden);
#   * parent -> child: a Ring 3 parent holding only `spawn` passes
#     `child delta echo-7` through spawn_args to a child holding NOTHING; the
#     child exits 42 only after checking them, and the parent requires 42;
#   * six hostile blocks (too many, too long, control byte, space, empty
#     argument, missing terminator) are each refused with the documented
#     error and no child is created;
#   * without the Process capability spawn_args is refused at the gate and
#     audited, exactly like spawn_caps;
#   * pre-V0.10 syntax is unchanged: `run <path>` gives no arguments, and a
#     stray fourth token is an error rather than silently ignored.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'run /bin/args-probe - - -- alpha beta gamma',
    '--send', 'run /bin/args-probe -- a b c d e f g h i j k l m n o p q',
    '--send', 'bg /bin/args-probe spawn -- spawn-child',
    '--send', 'bg /bin/args-probe - -- spawn-child',
    '--send', 'run /bin/args-probe',
    '--send', 'run /bin/args-probe - /etc extra',
    '--send', 'shutdown',
    '--require', 'ARGS-COUNT n=3',
    '--require', 'ARGS-ITEM i=0 value=alpha',
    '--require', 'ARGS-ITEM i=1 value=beta',
    '--require', 'ARGS-ITEM i=2 value=gamma',
    '--require', 'ARGS-2BIG-OK',
    '--require', 'ARGS-FAULT-OK',
    '--require', 'ARGS-OK',
    '--require', 'run: /bin/args-probe: Exit(0)',
    '--require', 'run: /bin/args-probe: arguments rejected: too_many (limit 16 arguments, 512 bytes',
    '--require', 'ARGS-ITEM i=0 value=spawn-child',
    '--require', 'action=spawn_args cap=0x0 result=ok',
    '--require', 'argc=3',
    '--require', 'ARGS-ITEM i=0 value=child',
    '--require', 'ARGS-ITEM i=1 value=delta',
    '--require', 'ARGS-ITEM i=2 value=echo-7',
    '--require', 'ARGS-CHILD-OK n=3',
    '--require', 'ARGS-SPAWN-OK child_status=42',
    '--require', 'spawn_args refused reason=too_many len=34',
    '--require', 'spawn_args refused reason=too_long len=601',
    '--require', 'spawn_args refused reason=bad_byte len=6',
    '--require', 'spawn_args refused reason=bad_byte len=10',
    '--require', 'spawn_args refused reason=empty len=4',
    '--require', 'spawn_args refused reason=unterminated len=3',
    '--require', 'ARGS-HOSTILE-REFUSED case=too_many',
    '--require', 'ARGS-HOSTILE-REFUSED case=too_long',
    '--require', 'ARGS-HOSTILE-REFUSED case=non_printable',
    '--require', 'ARGS-HOSTILE-REFUSED case=space',
    '--require', 'ARGS-HOSTILE-REFUSED case=empty',
    '--require', 'ARGS-HOSTILE-REFUSED case=unterminated',
    '--require', 'ARGS-HOSTILE-OK refused=6',
    # A NULL zero-length buffer/block is valid and must not be touched (a
    # debug kernel used to panic forming an empty slice from NULL).
    '--require', 'ARGS-NULL-BLOCK-OK',
    '--require', 'ARGS-NULL-EMPTY-OK',
    '--require', 'bg: /bin/args-probe: exit=0',
    '--require', 'ARGS-SPAWN-DENIED call=spawn_args',
    '--require', 'action=spawn_args cap=0x0 result=denied',
    '--require', 'kind=process reason=no_handle',
    '--require', 'ARGS-COUNT n=0',
    '--require', 'program arguments go after --',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'ARGS-HOSTILE-ACCEPTED',
    '--forbid', 'ARGS-HOSTILE-WRONG-ERROR',
    '--forbid', 'ARGS-SPAWN-FAILED',
    '--forbid', 'ARGS-CHILD-MISMATCH',
    '--forbid', 'ARGS-FAILED',
    '--forbid', 'RING3-PANIC',
    '--forbid', 'value=q',
    '--timeout-secs', '240', '--label', 'args-bios')

Write-Output '=== QEMU interrupt routing: ACPI MADT + I/O APIC cutover + MSI-X (BIOS) ==='
# V0.9: the timer, PS/2 keyboard and mouse run through the I/O APIC on the
# routes the ACPI MADT declares (QEMU: IRQ0 -> GSI 2), and the 8259s plus the
# local APIC's LINT0 are masked. Every OTHER leg in this matrix already runs
# on the new path; this one asserts the path is what it claims to be:
#   * the routes read back from the I/O APIC, the PIC masks read back 0xff/0xff;
#   * the timer's tick rate is the same before and after the cutover (a line
#     delivered twice would double every quantum - which is exactly what PIT
#     mode 3 did through this path before the fix);
#   * adversarially: with the timer's I/O APIC entry masked the tick count
#     stops dead, and it moves again when unmasked - so nothing else is
#     delivering the timer;
#   * a one-shot local-APIC timer interrupt, and MSI-X raised by a real NVMe
#     block read's completion, still arrive.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net', '--nvme',
    '--expect', 'B190', '--expect', 'B200', '--expect', 'B210',
    '--send', 'acpi',
    '--send', 'irq',
    '--send', 'shutdown',
    '--require', '[ITISYOU:ACPI] rsdp_revision=0 root=RSDT',
    '--require', 'madt=ok fadt=ok s5=ok',
    '--require', 'irq0_gsi=2',
    '--require', 'lapic_ready id=0',
    '--require', 'ioapic id=0 base=0xfec00000 source=madt',
    '--require', 'ioapic_route isa=0 gsi=2 vector=0x20 trigger=edge polarity=high masked=false readback=ok',
    '--require', 'ioapic_route isa=1 gsi=1 vector=0x21',
    '--require', 'ioapic_route isa=12 gsi=12 vector=0x2c',
    '--require', 'pic_retired masks=0xff/0xff lint0=masked legacy_lines=ioapic',
    '--require', 'rate_preserved=true',
    '--require', 'irq: legacy_lines=ioapic pic_masks=0xff/0xff',
    '--require', 'masked_ticks=0',
    '--require', 'result=ioapic_only',
    '--forbid', 'result=FAILED',
    '--forbid', 'rate_preserved=false',
    '--require', 'irq: apic_timer delivered=true count=1 vector=0x41',
    '--require', 'msix_enabled dev=00:04.0 vector=0x42',
    '--require', 'irq: msix armed=true block_read=true delivered=true',
    '--require', 'spurious=0',
    '--timeout-secs', '240', '--label', 'irq-bios')

Write-Output '=== QEMU ACPI S5 power-off (BIOS + UEFI) ==='
# `poweroff` writes SLP_TYP|SLP_EN to the PM1 control block the FADT names,
# with SLP_TYP from the DSDT's \_S5_ - not QEMU's test-exit device, which is
# what `shutdown` uses. Success is QEMU exiting on its own after the request;
# if the machine were still running 500 ms later the kernel prints
# `poweroff: failed`, which is forbidden. BIOS (ACPI 1.0, RSDT, PIIX4 at
# 0x604) and UEFI (ACPI 2.0, XSDT, 0xb004) exercise two different table sets.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'poweroff',
    '--require', '[ITISYOU:POWER] s5 request pm1a_cnt=0x604 slp_typ=0',
    '--forbid', 'poweroff: failed',
    '--timeout-secs', '120', '--label', 'acpi-poweroff-bios')
if (Test-Path 'target/images/itisyou-kernel-uefi.img') {
    & $runner @('--image', 'target/images/itisyou-kernel-uefi.img', '--uefi',
        '--expect', 'B210',
        '--send', 'poweroff',
        '--require', 'root=XSDT',
        '--require', '[ITISYOU:POWER] s5 request pm1a_cnt=0xb004 slp_typ=0',
        '--forbid', 'poweroff: failed',
        '--timeout-secs', '120', '--label', 'acpi-poweroff-uefi')
}

Write-Output '=== QEMU userspace filesystem writes (BIOS, two boots) ==='
# V0.8 capability-scoped Ring 3 writes to the persistent ITFS store. Boot 1
# walks the whole contract and boot 2 (a fresh guest, same disk) proves the
# write survived a real reboot through the USERSPACE path - a different claim
# from the V0.4 kernel-side persistence test, because the capability check,
# the sandbox check, the store-name parsing and the atomic commit all sit
# between the program and the disk.
#
# The error paths matter as much as the successes: five refusals, each for a
# different reason, so one over-broad check cannot pass by accident. And
# `fs-write-denied` holds `fs_read` and nothing else, so its refusals prove
# the WRITE right specifically - it can still read and list the same store.
#
# V0.11: the writer also tries a name with a line break in it (AUDIT11-002:
# echoed into the audit detail, it forged a kernel marker line) and the
# kernel-owned files (SEC11-001): the audit trail and the package store. An
# application is installed first, so the commit marker the writer tries to
# delete is real, and `pkg list` shows it is still active afterwards.
$fsDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-fswrite-test.img'
Remove-Item $fsDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $fsDisk,
    '--expect', 'B190',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'pkg list',
    '--send', 'store cat echo.txt',
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
    # Kernel-owned files: refused and audited for each call, and the commit
    # marker the program tried to delete still makes v1 the active version.
    '--require', 'FSWRITE-KERNEL-OWNED-REFUSED',
    '--require', 'action=fs_write cap=0x80 result=denied',
    '--require', 'action=fs_delete cap=0x80 result=denied',
    '--require', 'action=fs_read cap=0x10 result=denied',
    '--require', 'reason=kernel_owned',
    '--require', 'hello-app: active=Some(1) previous=None staged=None',
    # The line-break name is refused before anything echoes it, and contents
    # a program wrote are echoed as one line with the marker neutralized.
    '--require', 'store: echo.txt = line1\x0a[RING3-U:AUDIT] forged-content',
    '--forbid', '[ITISYOU:AUDIT] forged',
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

Write-Output '=== QEMU ITFS space reclamation (BIOS, two boots) ==='
# V0.10: freed extents are reused. The disk is 8 KiB - 16 blocks, 14 of them
# data - and boot 1 writes 21 one-block files' worth into it: a kept file,
# then the same file overwritten 20 times through the real NVMe path. The
# V0.9 bump allocator leaked every old extent and would have refused the
# 15th write with NoSpace; here every write must succeed and the report must
# show just the two live blocks in use. Boot 2 (a fresh guest, same disk)
# must read back the last revision, the kept file, and the identical report.
# The report's numbers are predicted by the host test
# `qemu_reclaim_leg_space_report_is_predicted`, not copied from a run.
$reclaimDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-reclaim-test.img'
Remove-Item $reclaimDisk -ErrorAction SilentlyContinue
[System.IO.File]::WriteAllBytes($reclaimDisk, (New-Object byte[] 8192))
$reclaimReport = '[ITISYOU:FS] reclaim data_blocks=14 used=2 free=12 pinned=1 largest_run=11 files=2 generation=22'
$reclaimSends = @('--send', 'store put keep.txt kept-across-reclaim')
foreach ($rev in 1..20) {
    $reclaimSends += @('--send', ('store put churn.txt rev{0:D2}-of-20-reclaim-cycle' -f $rev))
}
& $runner (@('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $reclaimDisk,
    '--expect', 'B210') + $reclaimSends + @(
    '--send', 'store df',
    '--send', 'store cat churn.txt',
    '--send', 'store ls',
    '--send', 'shutdown',
    '--require', 'store: put name=churn.txt bytes=25',
    '--require', $reclaimReport,
    '--require', 'store: churn.txt = rev20-of-20-reclaim-cycle',
    '--require', 'store: files=2 generation=22',
    '--forbid', 'failed: Fs(',
    '--forbid', 'failed: Block(',
    '--timeout-secs', '240', '--label', 'fs-reclaim-churn'))
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $reclaimDisk,
    '--expect', 'B210',
    '--send', 'store df',
    '--send', 'store cat churn.txt',
    '--send', 'store cat keep.txt',
    '--send', 'store ls',
    '--send', 'shutdown',
    '--require', $reclaimReport,
    '--require', 'store: churn.txt = rev20-of-20-reclaim-cycle',
    '--require', 'store: keep.txt = kept-across-reclaim',
    '--require', 'store: files=2 generation=22',
    '--forbid', 'failed: Fs(',
    '--forbid', 'failed: Block(',
    '--timeout-secs', '240', '--label', 'fs-reclaim-persist')
Remove-Item $reclaimDisk -ErrorAction SilentlyContinue

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
    # V0.11: `audit verify` checks the file without adopting it.
    '--require', 'trail_checked status=TAMPERED reason=chain',
    '--timeout-secs', '240', '--label', 'audit-persist-tamper')
Remove-Item $auditDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU audit trail across boots and past the window (BIOS, three boots) ==='
# AUDIT11-001. Until V0.11 the trail stored only the in-memory ring's records
# (64, this boot's only) under a head covering every record ever made, so the
# next boot reported a trail nobody touched as TAMPERED whenever the ring had
# dropped a record or an earlier boot's trail had been recovered. Boot 1 makes
# more records than the stored window holds (13 per fs-writer run - three
# writes and a delete, nine refusals - x 11 = 143, plus the boot's own), so
# the save must move `base` off genesis. Boot 2 recovers it, adds
# records, checks the file mid-boot (read-only: before V0.11 this re-ran
# recovery and dropped the unsaved records from the chain) and saves again.
# Boot 3 must find the second boot's trail verified.
$windowDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-audit-window.img'
Remove-Item $windowDisk -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $windowDisk,
    '--expect', 'B210',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'audit save',
    '--send', 'audit verify',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=absent',
    '--require', 'trail_saved records=128 ',
    '--require', 'trail_checked status=verified reason=none records=128 ',
    '--require', 'audit: chain boot=0 saved_records=128 window=128 ',
    # More records than fit: the base must have moved past genesis.
    '--forbid', 'base=0000000000000000000000000000000000000000000000000000000000000000',
    '--forbid', 'FSWRITE-FAILED',
    '--timeout-secs', '300', '--label', 'audit-window-write')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $windowDisk,
    '--expect', 'B210',
    '--send', 'run /bin/fs-writer fs_read,fs_write',
    '--send', 'audit verify',
    '--send', 'audit save',
    '--send', 'audit verify',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=verified records=128 ',
    '--require', 'trail_checked status=verified reason=none records=128 ',
    '--require', 'trail_saved records=128 ',
    '--require', 'audit: chain boot=1 saved_records=128 window=128 ',
    '--forbid', 'status=TAMPERED',
    '--timeout-secs', '300', '--label', 'audit-window-continue')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $windowDisk,
    '--expect', 'B210',
    '--send', 'audit',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=verified records=128 ',
    '--require', 'audit: chain boot=2 ',
    '--forbid', 'status=TAMPERED',
    '--timeout-secs', '300', '--label', 'audit-window-verify')
Remove-Item $windowDisk -ErrorAction SilentlyContinue

Write-Output '=== QEMU audit anchoring against a witness off the disk (BIOS, four boots) ==='
# The chain is unkeyed, so an attacker who rewrites the WHOLE trail can write
# one that verifies - in the limit a valid EMPTY trail that erases the history.
# The witness (the runner, over QEMU's user-mode network, persisting to a file
# across the separate boots) keeps a copy of the saved head the disk cannot
# reach. Boot 1 saves and anchors; boot 2 matches; boot 3 replaces the trail
# with a valid empty one; boot 4's recovery calls the forgery VERIFIED (the
# weakness) and the anchor check calls it MISMATCH (the defence).
$anchorDisk = Join-Path $env:ITISYOU_SCRATCH 'itisyou-anchor-test.img'
$anchorWitness = Join-Path $env:ITISYOU_SCRATCH 'itisyou-anchor-witness.txt'
foreach ($f in @($anchorDisk, $anchorWitness)) { if (Test-Path $f) { Remove-Item -LiteralPath $f } }
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user', '--nvme-persist', $anchorDisk, '--audit-witness', $anchorWitness,
    '--expect', 'B210',
    '--send', 'pkg install /pkgs/hello-app-1.itpkg',
    '--send', 'audit save',
    '--send', 'audit anchor 10.0.2.2 {WITNESS_PORT}',
    '--send', 'shutdown',
    '--require', 'trail_saved records=',
    '--require', 'witness_ack=ok',
    '--require', '[HOST:WITNESS] anchored count=',
    '--timeout-secs', '180', '--label', 'audit-anchor-save')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user', '--nvme-persist', $anchorDisk, '--audit-witness', $anchorWitness,
    '--expect', 'B210',
    '--send', 'audit check-anchor 10.0.2.2 {WITNESS_PORT}',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=verified',
    '--require', 'anchor_check result=MATCH',
    '--timeout-secs', '180', '--label', 'audit-anchor-match')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--nvme-persist', $anchorDisk,
    '--expect', 'B210',
    '--send', 'store put audit.log itisyou-audit v1 boot=0 count=0 head=0000000000000000000000000000000000000000000000000000000000000000',
    # V0.11: while the system runs, the running kernel is the reference - a
    # replaced trail is caught even though its chain is valid.
    '--send', 'audit verify',
    '--send', 'shutdown',
    '--require', 'trail_checked status=TAMPERED reason=replaced records=0 ',
    '--timeout-secs', '180', '--label', 'audit-anchor-forge')
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--net-user', '--nvme-persist', $anchorDisk, '--audit-witness', $anchorWitness,
    '--expect', 'B210',
    '--send', 'audit check-anchor 10.0.2.2 {WITNESS_PORT}',
    '--send', 'shutdown',
    '--require', 'trail_recovered status=verified records=0 head=0000000000000000000000000000000000000000000000000000000000000000',
    '--require', 'trail_count=0 trail_head=0000000000000000000000000000000000000000000000000000000000000000',
    '--require', 'anchor_check result=MISMATCH',
    '--require', 'action=audit_anchor_mismatch cap=0x0 result=denied',
    '--forbid', 'anchor_check result=MATCH',
    '--timeout-secs', '180', '--label', 'audit-anchor-detect')
foreach ($f in @($anchorDisk, $anchorWitness)) { if (Test-Path $f) { Remove-Item -LiteralPath $f } }

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

Write-Output '=== QEMU kernel stack guard: overflow is caught, not silent (BIOS) ==='
# V0.9: the RSP0 stack (interrupts and exceptions from Ring 3) and the
# double-fault IST have an unmapped guard page (the syscall stack's guard is
# V0.10's, next leg but one). The console deliberately overflows the RSP0
# stack; the guard must turn that into a double fault that
# names the stack — the first TCP integration showed that without it an
# overflow silently corrupted the capability table.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B080', '--expect', 'B200',
    '--send', 'panic-test stack-overflow',
    '--expect-panic',
    '--require', 'stack_guard stack=priv',
    '--require', 'stack_guard stack=ist_double_fault',
    '--require', 'armed=true unmapped=true',
    '--require', 'kernel_stack_overflow stack=priv',
    '--require', 'guard_hit=true',
    '--require', 'kernel stack overflow: stack=priv hit its guard page',
    '--forbid', 'armed=false',
    '--timeout-secs', '90', '--label', 'stack-guard-bios')

Write-Output '=== QEMU line-atomic Ring 3 output; kernel markers unforgeable (BIOS) ==='
# V0.10 OUT10-001/002: parent and child each write a line one byte per
# syscall, yielding between bytes, co-scheduled with the background
# services. Both lines must arrive whole, and a line imitating a kernel
# marker must come out rewritten. (Unbuffered, the lines interleave byte by
# byte and kernel markers land mid-line.)
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B200',
    '--send', 'bg /bin/line-probe spawn',
    '--send', 'shutdown',
    '--require', 'LINEPROBE-A-0123456789abcdefghijklmnopqrstuv',
    '--require', 'LINEPROBE-B-0123456789abcdefghijklmnopqrstuv',
    '--require', '[RING3-U:SVC] spoofed-by-ring3',
    '--require', 'LINEPROBE-OK',
    '--forbid', '[ITISYOU:SVC] spoofed-by-ring3',
    '--forbid', 'LINEPROBE-FAILED',
    '--timeout-secs', '180', '--label', 'line-atomic-bios')

Write-Output '=== QEMU kernel copies refuse read-only user memory (BIOS) ==='
# V0.10 SEC10-001: a program holding NO capability points `args` and
# `cap_list` at its own read-only code and data. The kernel must refuse with
# ERR_FAULT. (Before the fix it wrote there in Ring 0 and the write-protect
# fault panicked the kernel.)
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B200',
    '--send', 'run /bin/uaccess-probe - - -- probe',
    '--send', 'shutdown',
    '--require', 'UACCESS-ARGS-RO-REFUSED',
    '--require', 'UACCESS-CAPLIST-RO-REFUSED',
    '--require', 'UACCESS-RW-OK',
    '--require', 'UACCESS-OK',
    '--require', 'run: /bin/uaccess-probe: Exit(0)',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'UACCESS-FAILED',
    '--timeout-secs', '120', '--label', 'uaccess-bios')

Write-Output '=== QEMU process model: parent-only wait, wait_nohang, sleep, orphans, ps (BIOS) ==='
# V0.10 PROC10-002: a sibling may not collect another process's child;
# wait_nohang and sleep work; a parent that exits without waiting leaves
# an orphan that the kernel reaps when it ends (after `busy 500` no
# proc-probe slot, live or zombie, may remain in `ps`). Every marker is
# pid-agnostic.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'bg /bin/proc-probe spawn -- all',
    '--send', 'bg /bin/proc-probe spawn,service -- svc-report',
    '--send', 'bg /bin/proc-probe spawn -- svc-denied',
    '--send', 'busy 500',
    '--send', 'ps',
    '--send', 'kill 999',
    '--send', 'shutdown',
    '--require', 'PROCPROBE-FOREIGN-WAIT-REFUSED pid=',
    '--require', 'PROCPROBE-FOREIGN-NOHANG-REFUSED pid=',
    '--require', 'PROCPROBE-SIBLING-REFUSED-OK',
    '--require', 'action=wait_foreign cap=0x1 result=denied',
    '--require', 'PROCPROBE-NOCHILD-OK',
    '--require', 'PROCPROBE-NOHANG-AGAIN-OK',
    '--require', 'PROCPROBE-NOHANG-REAPED status=7',
    '--require', 'PROCPROBE-ANY-REAPED status=7',
    '--require', 'PROCPROBE-SLEEP-OK requested=50',
    '--require', 'PROCPROBE-SLEEP-BOUND-OK',
    '--require', 'PROCPROBE-ORPHAN-SPAWNED',
    '--require', 'PROCPROBE-OK',
    '--require', 'bg: /bin/proc-probe: exit=0',
    '--require', '[ITISYOU:PROC] reparent child=',
    '--require', 'INIT-REAPED-ORPHAN pid=',
    '--require', 'PROCPROBE-ORPHAN-CHILD-DONE',
    '--require', 'path=/bin/tickd state=runnable',
    '--require', 'ps: processes=',
    '--require', 'kill: pid=999: no such process',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'PROCPROBE-FAILED',
    '--forbid', 'PROCPROBE-SLEEP-SHORT',
    '--forbid', 'path=/bin/proc-probe',
    '--require', 'PROCPROBE-SVCREPORT-NOT-CHILD-OK',
    '--require', 'PROCPROBE-SVCREPORT-RESERVED-OK',
    '--require', 'PROCPROBE-SVCREPORT-NOT-OWNER-OK',
    '--require', 'PROCPROBE-SVCREPORT-READY-REFUSED-OK',
    '--require', 'PROCPROBE-SVCREPORT-ACCEPTED-OK',
    '--require', 'PROCPROBE-SVCREPORT-OK',
    '--require', 'init=false reason=not_child',
    '--require', 'init=false reason=reserved_name',
    '--require', 'init=false reason=not_owner',
    '--require', 'init=false reason=not_init',
    '--require', 'bg_start name=probe-svc pid=',
    '--require', 'bg_done name=probe-svc restarts=0 supervisor=',
    '--require', 'PROCPROBE-SVCREPORT-DENIED-OK',
    '--require', 'action=svc_report cap=0x0 result=denied',
    '--forbid', 'init=true reason=',
    '--timeout-secs', '180', '--label', 'proc-model-bios')

Write-Output '=== QEMU userspace init: configuration and supervision (BIOS) ==='
# V0.10 INIT10-002: /sbin/init parses /etc/init.conf with the host-tested
# grammar (check mode: the shipped file, a dependency cycle, an unknown
# capability, and no filesystem capability), then starts and supervises a
# service from a test config (once mode), reporting it to the kernel's
# service table through svc_report.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'ps',
    '--send', 'cat /etc/init.conf',
    '--send', 'run /sbin/init fs_read - -- check /etc/init.conf',
    '--send', 'run /sbin/init fs_read - -- check /etc/init-tests/cycle.conf',
    '--send', 'run /sbin/init fs_read - -- check /etc/init-tests/badcap.conf',
    '--send', 'run /sbin/init - - -- check /etc/init.conf',
    '--send', 'bg /sbin/init spawn,service,fs_read -- once /etc/init-tests/demo.conf',
    '--send', 'svc',
    '--send', 'bg /bin/proc-probe spawn -- all',
    '--send', 'busy 500',
    '--send', 'run /bin/tick-client',
    '--send', 'kill 1',
    '--send', 'busy 1000',
    '--send', 'kill 2',
    '--send', 'ps',
    '--send', 'run /bin/tick-client',
    '--send', 'shutdown',
    '--require', 'service tickd /bin/tickd caps=ipc restart=always',
    '--require', 'INIT-CONFIG-OK path=/etc/init.conf services=3 order=tickd,flapd,inferd',
    '--require', 'INIT-CONFIG-ERROR path=/etc/init-tests/cycle.conf line=0 reason=cycle',
    '--require', 'INIT-CONFIG-ERROR path=/etc/init-tests/badcap.conf line=2 reason=unknown_capability',
    '--require', 'INIT-CONFIG-ERROR path=/etc/init.conf line=0 reason=read_denied',
    '--require', 'run: /sbin/init: Exit(2)',
    '--require', 'INIT-CONFIG path=/etc/init-tests/demo.conf services=1 order=demo',
    '--require', 'bg_start name=demo pid=',
    '--require', 'long_running=false supervisor=',
    '--require', 'ARGS-ITEM i=0 value=from-config',
    '--require', 'INIT-DONE name=demo',
    '--require', 'bg_done name=demo restarts=0 supervisor=',
    '--require', 'INIT-ONCE-DONE services=1',
    '--require', 'bg: /sbin/init: exit=0',
    '--require', 'demo  Done',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'ARGS-FAILED',
    '--forbid', 'INIT-USAGE',
    '--forbid', 'INIT-REPORT-REFUSED',
    '--require', '[ITISYOU:INIT] start pid=1 path=/sbin/init caps=0x413 sandbox=/etc',
    '--require', 'INIT-CONFIG path=/etc/init.conf services=3 order=tickd,flapd,inferd',
    '--require', 'pid=1 action=spawn cap=0x2 result=ok',
    '--require', 'pid=1 action=spawn cap=0x0 result=ok',
    '--require', 'bg_start name=tickd pid=2 caps=0x2 long_running=true supervisor=1',
    '--require', 'bg_start name=flapd pid=3 caps=0x0 long_running=true supervisor=1',
    '--require', 'bg_start name=inferd pid=4 caps=0x12 long_running=true supervisor=1',
    '--require', '[ITISYOU:INIT] ready pid=1 services=3',
    '--require', '[ITISYOU:INIT] settled ready=true',
    '--require', '  pid=1 ppid=0 path=/sbin/init state=',
    '--require', '  pid=2 ppid=1 path=/bin/tickd state=runnable',
    '--require', 'bg_failed name=flapd restarts=3 supervisor=1',
    '--require', 'INIT-REAPED-ORPHAN pid=',
    '--require', 'TICKC-OK passes=',
    '--require', 'kill: pid=1 path=/sbin/init state=killed',
    '--require', '[ITISYOU:INIT] died pid=1 status=killed',
    '--require', 'action=restart restarts=1',
    '--require', '[ITISYOU:INIT] restart pid=',
    '--require', 'caps=0x413 sandbox=/etc restarts=1',
    '--require', 'kill: pid=2: no such process',
    '--forbid', 'settled ready=false',
    '--forbid', 'init=true reason=',
    '--forbid', 'action=give_up',
    '--forbid', 'kill: pid=2 path=',
    '--forbid', 'TICKC-RECV-TIMEOUT',
    '--forbid', 'TICKC-SEND-TIMEOUT',
    '--timeout-secs', '240', '--label', 'init-bios')

Write-Output '=== QEMU Ring 3 shell with the console input (BIOS) ==='
# V0.10 SHELL10-001: `rsh` hands the console's input to /bin/sh, which
# reads lines with console_read (syscall 40), runs builtins and programs
# (a genuinely blocking wait), and on `exit` the input comes back to the
# kernel console. Forbids prove which shell read each line: the kernel
# never saw the sh-only lines, and sh never saw the ones after exit. A
# process that does not own the input is refused.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'bg /bin/proc-probe - -- console-read',
    '--send', 'rsh',
    '--send', 'rsh-only-builtin',
    '--send', 'pid',
    '--send', 'run /bin/args-probe - -- x y',
    '--send', 'run /bin/tick-client',
    '--send', 'exit',
    '--send', 'echo back-in-kernel',
    '--send', 'version',
    '--send', 'shutdown',
    '--require', 'PROCPROBE-CONSOLE-READ-REFUSED',
    '--require', 'action=console_read cap=0x0 result=denied',
    '--require', '[ITISYOU:CONSOLE] owner=',
    '--require', 'reason=granted',
    '--require', 'RSH-READY pid=',
    '--require', 'RSH-BUILTIN-OK',
    '--require', 'rsh: pid=',
    '--require', 'ARGS-COUNT n=2',
    '--require', 'ARGS-ITEM i=1 value=y',
    '--require', 'rsh: /bin/args-probe: exit=0',
    '--require', 'TICKC-OK passes=',
    '--require', 'rsh: /bin/tick-client: exit=0',
    '--require', 'RSH-EXIT code=0',
    '--require', '[ITISYOU:CONSOLE] owner=kernel reason=owner_exit',
    '--require', 'rsh: /bin/sh: exit=0',
    '--require', 'back-in-kernel',
    # The kernel console's `version` line, without pinning a version (the
    # V0.11 dev bump missed this pin and failed the leg).
    '--require', '(x86_64, QEMU, pre-alpha)',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'unknown command: rsh-only-builtin',
    '--forbid', 'unknown command: pid',
    '--forbid', 'run: /bin/args-probe: Exit(',
    '--forbid', 'RSH-READ-ERROR',
    '--forbid', 'rsh: unknown command: echo',
    '--forbid', 'rsh: unknown command: version',
    '--timeout-secs', '180', '--label', 'rsh-bios')

Write-Output '=== QEMU desktop: a Ring 3 app, click-to-focus, keys to the focused window (BIOS) ==='
# V0.10 DESK10-001: `desktop /bin/gui-echo` puts a persistent Ring 3 app on
# the live desktop. The cursor is parked in the corner, then moved onto
# the app and clicked (it gets the focus and a focus-in event); `a` goes to
# the app, not the desktop; a click on the desktop's own window moves the
# focus back (the app gets a focus-out) and `b` stays with the desktop.
$focusShot = Join-Path (Resolve-Path 'artifacts/qemu') 'desktop-focus.ppm'
Remove-Item $focusShot -ErrorAction SilentlyContinue
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', 'desktop /bin/gui-echo',
    '--inject-after', 'GUIECHO-READY',
    '--inject-delay-ms', '1000',
    '--monitor-cmd', 'mouse_move -300 -200',
    '--monitor-cmd', 'mouse_move -300 -200',
    '--monitor-cmd', 'mouse_move -300 -200',
    '--monitor-cmd', 'mouse_move 150 140',
    '--monitor-cmd', 'mouse_button 1',
    '--monitor-cmd', 'mouse_button 0',
    '--monitor-cmd', 'sendkey a',
    '--monitor-cmd', 'mouse_move 200 100',
    '--monitor-cmd', 'mouse_move 200 100',
    '--monitor-cmd', 'mouse_button 1',
    '--monitor-cmd', 'mouse_button 0',
    '--monitor-cmd', 'sendkey b',
    '--monitor-cmd', "screendump $focusShot",
    '--monitor-cmd', 'sendkey esc',
    '--require', '[ITISYOU:MODE] desktop',
    '--require', 'DESKTOP-APP pid=',
    '--require', 'GUIECHO-READY win=',
    '--require', 'DESKTOP-FOCUS win=2 owner=',
    '--require', 'GUIECHO-FOCUS-IN',
    '--require', 'DESKTOP-ROUTE key=a win=2',
    '--require', 'GUIECHO-KEY a',
    '--require', 'GUIECHO-FOCUS-OUT',
    '--require', 'DESKTOP-FOCUS win=1 owner=0',
    '--require', 'DESKTOP-KEY b',
    '--require', 'DESKTOP-INPUT-VERIFIED',
    '--forbid', 'GUIECHO-KEY b',
    '--forbid', 'GUIECHO-EVENT-ERROR',
    '--forbid', 'GUIECHO-FAILED',
    '--forbid', 'DESKTOP-TIMEOUT',
    '--forbid', 'load_failed',
    '--timeout-secs', '150', '--label', 'desktop-focus-bios')

Write-Output '=== QEMU always-on co-scheduling (BIOS) ==='
# V0.10 SCHED10-001: background processes run in bounded slices at the
# audited safe points - the idle prompt (held idle by @pause), job waits,
# busy kernel waits (busy, net poll), between quanta of a foreground
# program - never in an ISR or a syscall. In-leg control: with scheduling
# paused, `busy` starves the background (others_progressed=false).
& $runner @('--image', 'target/images/itisyou-kernel-bios.img', '--net-user',
    '--expect', 'B210',
    '--send', '@pause 1500',
    '--send', 'sched',
    '--send', 'sched pause',
    '--send', 'busy 1000',
    '--send', 'sched last',
    '--send', 'sched resume',
    '--send', 'busy 2000',
    '--send', 'sched last',
    '--send', 'run /bin/tick-client',
    '--send', 'run /bin/burn - - -- 150',
    '--send', 'sched last',
    '--send', 'net poll 1500',
    '--send', 'sched last',
    '--send', 'bg /bin/line-probe spawn',
    '--send', 'run /bin/args-probe spawn -- spawn-child',
    '--send', 'run /bin/proc-probe - -- sleep',
    '--send', 'sched',
    '--send', 'shutdown',
    '--require', '[ITISYOU:SCHED] enabled period_ticks=5',
    '--require', '[ITISYOU:SCHED] always_on=true paused=false period_ticks=5',
    '--require', 'busy: ms=1000 paused=true others_progressed=false other_quanta=0',
    '--require', '[ITISYOU:SCHED] window cmd=busy paused=true judged=true starved=true',
    '--require', 'busy: ms=2000 paused=false others_progressed=true',
    '--require', '[ITISYOU:SCHED] window cmd=busy paused=false judged=true starved=false',
    '--require', 'TICKC-OK passes=',
    '--require', 'run: /bin/tick-client: Exit(0)',
    '--require', 'BURN-OK ticks=150 yields=0',
    '--require', 'run: /bin/burn: Exit(0)',
    '--require', '[ITISYOU:SCHED] window cmd=run paused=false judged=true starved=false',
    '--require', 'net: polled ms=1500',
    '--require', '[ITISYOU:SCHED] window cmd=net paused=false judged=true starved=false',
    '--require', 'LINEPROBE-A-0123456789abcdefghijklmnopqrstuv',
    '--require', 'LINEPROBE-OK',
    '--require', 'lock_skips=0 ',
    '--require', 'stack_margin_ok=true',
    '--require', 'ARGS-SPAWN-OK child_status=42',
    '--require', 'run: /bin/args-probe: Exit(0)',
    '--require', 'PROCPROBE-SLEEP-OK requested=50',
    '--require', 'run: /bin/proc-probe: Exit(0)',
    '--require', 'shutting down (QEMU exit)',
    '--forbid', 'ARGS-SPAWN-FAILED',
    '--forbid', 'PROCPROBE-SLEEP-SHORT',
    '--forbid', 'paused=false judged=true starved=true',
    '--forbid', 'idle_slices=0 ',
    '--forbid', 'TICKC-RECV-TIMEOUT',
    '--forbid', 'TICKC-SEND-TIMEOUT',
    '--forbid', 'stack_margin_ok=false',
    '--forbid', 'LINEPROBE-FAILED',
    '--timeout-secs', '240', '--label', 'sched-always-on-bios')

Write-Output '=== QEMU always-on co-scheduling: what pause stops (BIOS) ==='
# `sched pause` stops background slices at busy and idle points (`busy`
# starves the background), but never the console's own job: a job wait
# slices everything runnable, so a foreground client still gets its
# service's reply - the console can never deadlock on its own job. (Until
# S7 moved `run` into the process table, this leg was the control showing
# that a paused foreground `run` starved the service: TICKC-RECV-TIMEOUT.)
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B200',
    '--send', 'sched pause',
    '--send', 'busy 1000',
    '--send', 'run /bin/tick-client',
    '--send', 'sched last',
    '--send', 'sched resume',
    '--send', 'shutdown',
    '--require', '[ITISYOU:SCHED] paused=true',
    '--require', 'busy: ms=1000 paused=true others_progressed=false other_quanta=0',
    '--require', 'TICKC-OK passes=',
    '--require', 'run: /bin/tick-client: Exit(0)',
    '--require', '[ITISYOU:SCHED] window cmd=run paused=true',
    '--require', '[ITISYOU:SCHED] paused=false',
    '--forbid', 'TICKC-RECV-TIMEOUT',
    '--timeout-secs', '180', '--label', 'sched-pause-bios')

Write-Output '=== QEMU kernel SYSCALL stack guard (BIOS) ==='
# V0.10 HARD10-003: the stack every syscall runs on (the one V0.9's TCP bug
# overflowed) is guarded like RSP0 and the double-fault stack.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B080', '--expect', 'B200',
    '--send', 'panic-test syscall-stack-overflow',
    '--expect-panic',
    '--require', 'stack_guard stack=syscall',
    '--require', 'kernel_stack_overflow stack=syscall',
    '--require', 'kernel stack overflow: stack=syscall hit its guard page',
    '--forbid', 'armed=false',
    '--timeout-secs', '90', '--label', 'stack-guard-syscall-bios')

Write-Output '=== QEMU Ring 3 DF/AC never reach kernel code (BIOS) ==='
# V0.10 HARD10-002: a Ring 3 program sets DF and AC and is preempted many
# times (spin), then faults with them set; the kernel counts every entry from
# Ring 3 that finds either flag still set. Before the fix: 75 and 38.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B200',
    '--send', 'run /bin/flags-probe',
    '--send', 'run /bin/flags-probe - - -- fault',
    '--send', 'harden',
    '--send', 'shutdown',
    '--require', 'FLAGSPROBE-SPIN df=1 ac=1',
    '--require', 'FLAGSPROBE-SPUN',
    '--require', 'FLAGSPROBE-FAULT-ARMED df=1 ac=1',
    '--require', 'vector=13 addr=0x0 contained=true',
    '--require', 'dirty_timer=0 dirty_landing=0',
    '--forbid', 'ring3_entry_flag_checks=0 ',
    '--forbid', 'user_set_seen=0 ',
    '--timeout-secs', '180', '--label', 'flags-hygiene-bios')

Write-Output '=== QEMU kernel TASK stack guard (BIOS) ==='
# V0.10: kernel task stacks live in guarded slots of a dedicated window. A
# task that recurses must end in a double fault that names it.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B080', '--expect', 'B200',
    '--send', 'panic-test task-stack-overflow',
    '--expect-panic',
    '--require', 'guard_unmapped=true; yielding to it',
    '--require', 'kernel_stack_overflow stack=task id=',
    '--require', 'guard_hit=true',
    '--require', 'kernel stack overflow: stack=task id=',
    '--forbid', 'the overflowing task returned',
    '--timeout-secs', '90', '--label', 'stack-guard-task-bios')

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

Write-Output '=== QEMU V0.11: the diagnostic model the build trained (BIOS) ==='
# MODEL11-001 (ADR-0024). The build trains the model from ai/scenarios.txt,
# refuses to build on a stale pin, ships /etc/ai/diag.model and compiles its
# SHA-256 into the kernel; at boot the kernel checks that the initramfs copy
# is the one it was built with and that it decodes. The digest required here
# is the pin in ai/diag.model.sha256.
$modelPin = (Get-Content ai/diag.model.sha256 -TotalCount 1).Trim()
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B110',
    '--send', 'shutdown',
    '--require', "[ITISYOU:AI] model sha256=$modelPin bytes=276 initramfs_match=true decode=ok",
    '--forbid', 'initramfs_match=false',
    '--timeout-secs', '120', '--label', 'ai-model-bios')

Write-Output '=== QEMU V0.11: the approved system view (BIOS) ==='
# VIEW11-001 (ADR-0024). sys_view (42) serves a fixed, allow-listed record of
# counts and service rows, and only to a process the console granted
# `sys_view` by name: refused with no capabilities, and refused to a `run`
# without a caps list (the legacy default no longer carries it). Granted, the
# record decodes with the host-tested sysview module and tells the truth -
# flapd failed after 3 restarts, tickd running, both reported by init, the
# scheduler's pause - and the kernel remembers what it served (the
# provenance a proposal will cite). A buffer one byte short is refused whole.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', '@pause 1500',
    '--send', 'run /bin/ai-probe - - -- view-denied',
    '--send', 'run /bin/ai-probe -- view-denied',
    '--send', 'run /bin/ai-probe sys_view - -- view',
    '--send', 'sched pause',
    '--send', 'run /bin/ai-probe sys_view - -- view',
    '--send', 'sched resume',
    '--send', 'shutdown',
    '--require', 'AIPROBE-VIEW-DENIED err=perm',
    '--require', 'action=sys_view cap=0x0 result=denied',
    '--require', 'kind=sysadmin reason=no_handle',
    # The legacy default holds a sysadmin handle (ADMIN) but not READ.
    '--require', 'kind=sysadmin reason=scope_denied',
    '--require', 'AIPROBE-VIEW-SMALL-OK',
    '--require', '[ITISYOU:AI] view_served pid=',
    '--require', 'AIPROBE-VIEW sched paused=0 always_on=1 ',
    '--require', 'AIPROBE-VIEW sched paused=1 always_on=1 ',
    '--require', 'AIPROBE-VIEW service=tickd state=running restarts=0 by_init=1 ',
    '--require', 'AIPROBE-VIEW service=flapd state=failed restarts=3 by_init=1 ',
    '--require', 'AIPROBE-VIEW features=',
    '--require', 'AIPROBE-VIEW-OK',
    '--forbid', 'AIPROBE-VIEW-LEAK',
    '--forbid', 'AIPROBE-VIEW-DECODE-FAILED',
    '--forbid', 'AIPROBE-FAILED',
    '--forbid', 'RING3-PANIC',
    '--timeout-secs', '180', '--label', 'ai-view-bios')

Write-Output '=== QEMU V0.11: inference in Ring 3 (BIOS) ==='
# INFER11-001 (ADR-0024). /sbin/init starts /bin/inferd as its third service
# (tickd and flapd keep pids 2 and 3); it loads the model the build trained
# and answers over IPC channels 6 and 7. Its check mode refuses each hostile
# fixture for its own reason. The probe asks it about the real view and
# recomputes the answer from the model file itself: Ring 3 inference is
# exactly the host-tested arithmetic on the shipped bytes. Job-wait slices
# run while the scheduler is paused, so inferd answers then too.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', '@pause 1500',
    '--send', 'run /bin/inferd fs_read /etc -- check /etc/ai/diag.model',
    '--send', 'run /bin/inferd fs_read /etc -- check /etc/ai/fixtures/bad-magic.model',
    '--send', 'run /bin/inferd fs_read /etc -- check /etc/ai/fixtures/truncated.model',
    '--send', 'run /bin/inferd fs_read /etc -- check /etc/ai/fixtures/wrong-dims.model',
    '--send', 'run /bin/inferd fs_read /etc -- check /etc/ai/fixtures/absurd-weight.model',
    '--send', 'run /bin/ai-probe sys_view,ipc,fs_read /etc -- infer',
    '--send', 'sched pause',
    '--send', 'run /bin/ai-probe sys_view,ipc,fs_read /etc -- infer',
    '--send', 'sched resume',
    '--send', 'shutdown',
    '--require', 'bg_start name=inferd pid=4 caps=0x12 long_running=true supervisor=1',
    '--require', 'INFERD-READY model=e0fc6642563aba99 channel=6',
    '--require', 'INFERD-MODEL-OK path=/etc/ai/diag.model sha256=e0fc6642563aba99',
    '--require', 'INFERD-MODEL-REFUSED path=/etc/ai/fixtures/bad-magic.model reason=bad_magic',
    '--require', 'INFERD-MODEL-REFUSED path=/etc/ai/fixtures/truncated.model reason=bad_length',
    '--require', 'INFERD-MODEL-REFUSED path=/etc/ai/fixtures/wrong-dims.model reason=bad_dimensions',
    '--require', 'INFERD-MODEL-REFUSED path=/etc/ai/fixtures/absurd-weight.model reason=weight_out_of_range',
    '--require', 'AIPROBE-INFER conditions=service_failed match=true model_match=true ',
    '--require', 'AIPROBE-INFER conditions=service_failed,scheduler_paused match=true model_match=true ',
    '--require', 'INFERD-SERVED n=2',
    '--forbid', 'match=false',
    '--forbid', 'AIPROBE-INFER-TIMEOUT',
    '--forbid', 'AIPROBE-FAILED',
    '--forbid', 'INFERD-BAD-REQUEST',
    '--forbid', 'RING3-PANIC',
    '--timeout-secs', '180', '--label', 'ai-infer-bios')

Write-Output '=== QEMU V0.11: the diagnostic agent on real situations (BIOS) ==='
# AGENT11-001 and KB11-001 (ADR-0024). /bin/agent reads the view, asks inferd,
# looks up each condition's runbook in /etc/ai/kb and prints the diagnosis and
# the action the runbook names - it files nothing yet and holds no authority.
# Three situations the leg constructs, each with its EXACT condition set: the
# booted system (flapd failed), the scheduler paused, and a burst of denials
# (sandbox-probe run with no capabilities). Without the view capability the
# agent is refused.
& $runner @('--image', 'target/images/itisyou-kernel-bios.img',
    '--expect', 'B210',
    '--send', '@pause 1500',
    '--send', 'run /bin/agent sys_view,ipc,fs_read /etc/ai -- diagnose',
    '--send', 'sched pause',
    '--send', 'run /bin/agent sys_view,ipc,fs_read /etc/ai -- diagnose',
    '--send', 'sched resume',
    '--send', 'run /bin/sandbox-probe -',
    '--send', 'run /bin/agent sys_view,ipc,fs_read /etc/ai -- diagnose',
    '--send', 'run /bin/agent - /etc/ai -- diagnose',
    '--send', 'shutdown',
    '--require', 'AGENT-DIAGNOSIS conditions=service_failed model=e0fc6642563aba99 ',
    '--require', 'AGENT-DIAGNOSIS conditions=service_failed,scheduler_paused model=e0fc6642563aba99 ',
    '--require', 'AGENT-DIAGNOSIS conditions=service_failed,denial_burst model=e0fc6642563aba99 ',
    '--require', 'AGENT-RUNBOOK id=service_failed sha=',
    '--require', 'AGENT-RUNBOOK id=scheduler_paused sha=',
    '--require', 'AGENT-RUNBOOK id=denial_burst sha=',
    '--require', 'AGENT-ACTION id=service_failed action=retry-service target=flapd',
    '--require', 'AGENT-ACTION id=scheduler_paused action=resume-scheduler',
    '--require', 'AGENT-ACTION id=denial_burst action=none',
    '--require', 'AGENT-VIEW-DENIED err=perm',
    '--forbid', 'AGENT-INFER-TIMEOUT',
    '--forbid', 'conditions=none ',
    '--forbid', 'AGENT-RUNBOOK-MISSING',
    '--forbid', 'RING3-PANIC',
    '--timeout-secs', '180', '--label', 'ai-diagnose-bios')

if ($anyFailed) { Write-Output 'TEST: FAILED'; exit 1 }
Write-Output 'TEST: OK'
exit 0
