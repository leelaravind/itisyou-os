/**
 * Boot-stage contract B000–B150.
 *
 * Mirrors crates/kernel-core/src/stage.rs — codes and descriptions are the
 * kernel's own strings. If stage.rs changes, update this list (stages are
 * append-only by contract; existing codes are never renumbered).
 *
 * `moduleId` links each stage to the status/current.json module whose work
 * makes the stage reachable, so the visualization inherits real statuses and
 * can never claim progress the status file does not record.
 */
export interface BootStage {
  code: string;
  describe: string;
  moduleId: string;
}

export const BOOT_STAGES: BootStage[] = [
  { code: 'B000', describe: 'firmware/loader handoff observed', moduleId: 'boot' },
  { code: 'B010', describe: 'kernel entry reached in 64-bit mode', moduleId: 'boot' },
  { code: 'B020', describe: 'early serial console ready', moduleId: 'serial' },
  { code: 'B030', describe: 'CPU baseline established', moduleId: 'cpu' },
  { code: 'B040', describe: 'boot memory map validated', moduleId: 'memory-physical' },
  { code: 'B050', describe: 'physical memory manager ready', moduleId: 'memory-physical' },
  { code: 'B060', describe: 'virtual memory abstraction ready', moduleId: 'memory-virtual' },
  { code: 'B070', describe: 'kernel heap ready', moduleId: 'heap' },
  { code: 'B080', describe: 'descriptor/exception layer ready', moduleId: 'interrupts' },
  { code: 'B090', describe: 'interrupt controller/timer ready', moduleId: 'interrupts' },
  { code: 'B100', describe: 'scheduler initialized', moduleId: 'scheduler' },
  { code: 'B110', describe: 'VFS/initramfs initialized', moduleId: 'vfs' },
  { code: 'B120', describe: 'input path initialized', moduleId: 'shell' },
  { code: 'B130', describe: 'shell/init task running', moduleId: 'shell' },
  { code: 'B140', describe: 'userspace transition ready', moduleId: 'ring3' },
  { code: 'B150', describe: 'V0.1 boot acceptance reached', moduleId: 'test-harness' },
];
