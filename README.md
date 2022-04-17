# linux-perf-data

This repo contains a parser for the perf.data format which is output by the Linux `perf` tool.

It also contains a `main.rs` which acts similarly to `perf script` and does symbolication, but with the advantage that it is much much faster than `perf script`.

The end goal of this project is to create a fast drop-in replacement for `perf script`, implementing just a basic subset of functionality, but having super fast symbolication. But that replacement will move to a separate repo. This repo should just contain a library crate for parsing the data.

## Acknowledgements

Some of the code in this repo was based on [**@koute**'s `not-perf` project](https://github.com/koute/not-perf/tree/20e4ddc2bf8895d96664ab839a64c36f416023c8/perf_event_open/src).

## Run

```
% cargo run --release -- perf.data
Hostname: ubuildu
OS release: 5.13.0-35-generic
Perf version: 5.13.19
Arch: x86_64
CPUs: 16 online (16 available)
Build IDs:
 - PID -1, DSO key Some("[kernel.kallsyms]"), build ID 101ecd8ba902186974b9d547f9bfa64b166b3bb9, filename [kernel.kallsyms]
 - PID -1, DSO key Some("dump_syms"), build ID 510d0a5c19eadf8043f203b4525be9be3dcb9554, filename /home/mstange/code/dump_syms/target/release/dump_syms
 - PID -1, DSO key Some("[vdso]"), build ID 0d82ee4bd7f9609c367095ba0bedf155b71cb058, filename [vdso]
 - PID -1, DSO key Some("libc.so.6"), build ID f0fc29165cbe6088c0e1adf03b0048fbecbc003a, filename /usr/lib/x86_64-linux-gnu/libc.so.6
0xffffffff92000000-0xffffffff93002507 Some(CodeId(101ecd8ba902186974b9d547f9bfa64b166b3bb9)) "[kernel.kallsyms]_text"
0xffffffffc007a000-0xffffffffc0085000 None "/lib/modules/5.13.0-35-generic/kernel/fs/autofs/autofs4.ko"
0xffffffffc008b000-0xffffffffc0097000 None "/lib/modules/5.13.0-35-generic/kernel/net/netfilter/x_tables.ko"
0xffffffffc009a000-0xffffffffc00a3000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/pinctrl/intel/pinctrl-cannonlake.ko"
0xffffffffc00a8000-0xffffffffc00b5000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/acpi/video.ko"
0xffffffffc00bb000-0xffffffffc00c3000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/wmi.ko"
0xffffffffc00cb000-0xffffffffc00d3000 None "/lib/modules/5.13.0-35-generic/kernel/net/ipv4/netfilter/ip_tables.ko"
0xffffffffc00ee000-0xffffffffc00fe000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/parport/parport.ko"
0xffffffffc0106000-0xffffffffc018f000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/drm.ko"
0xffffffffc0190000-0xffffffffc0195000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/acpi/acpi_tad.ko"
0xffffffffc019a000-0xffffffffc019e000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/macintosh/mac_hid.ko"
0xffffffffc019f000-0xffffffffc01a5000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/misc/mei/hdcp/mei_hdcp.ko"
0xffffffffc01a6000-0xffffffffc01aa000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-seq-device.ko"
0xffffffffc01af000-0xffffffffc01b5000 None "/lib/modules/5.13.0-35-generic/kernel/crypto/cryptd.ko"
0xffffffffc01b6000-0xffffffffc01bb000 None "/lib/modules/5.13.0-35-generic/kernel/sound/hda/snd-intel-sdw-acpi.ko"
0xffffffffc01be000-0xffffffffc01c3000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/char/lp.ko"
0xffffffffc01c9000-0xffffffffc01cd000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/crypto/crc32-pclmul.ko"
0xffffffffc01d1000-0xffffffffc01d5000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/input/sparse-keymap.ko"
0xffffffffc01d8000-0xffffffffc01e2000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/asus-wmi.ko"
0xffffffffc01e8000-0xffffffffc01ec000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/mfd/mfd-aaeon.ko"
0xffffffffc01ed000-0xffffffffc01f3000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/char/ppdev.ko"
0xffffffffc01f4000-0xffffffffc01ff000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/parport/parport_pc.ko"
0xffffffffc0200000-0xffffffffc0204000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/video/fbdev/core/sysimgblt.ko"
0xffffffffc0207000-0xffffffffc020b000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/kernel/msr.ko"
0xffffffffc0213000-0xffffffffc0218000 None "/lib/modules/5.13.0-35-generic/kernel/net/sched/sch_fq_codel.ko"
0xffffffffc021d000-0xffffffffc0222000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/usb/host/xhci-pci-renesas.ko"
0xffffffffc0225000-0xffffffffc022e000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/ata/libahci.ko"
0xffffffffc0242000-0xffffffffc0247000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/i2c/i2c-smbus.ko"
0xffffffffc025e000-0xffffffffc028b000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/acpi/acpi_pad.ko"
0xffffffffc028c000-0xffffffffc0291000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/events/rapl.ko"
0xffffffffc02ad000-0xffffffffc02f1000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/net/ethernet/intel/e1000e/e1000e.ko"
0xffffffffc02fb000-0xffffffffc02ff000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soundcore.ko"
0xffffffffc0303000-0xffffffffc030c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/i2c/busses/i2c-i801.ko"
0xffffffffc0312000-0xffffffffc0318000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/usb/host/xhci-pci.ko"
0xffffffffc031d000-0xffffffffc0327000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/ata/ahci.ko"
0xffffffffc0388000-0xffffffffc038c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/leds/trigger/ledtrig-audio.ko"
0xffffffffc038d000-0xffffffffc0391000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/input/input-leds.ko"
0xffffffffc0392000-0xffffffffc0396000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/snd-soc-acpi.ko"
0xffffffffc039e000-0xffffffffc03a2000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-pcm-dmaengine.ko"
0xffffffffc03a3000-0xffffffffc03b7000 None "/lib/modules/5.13.0-35-generic/kernel/sound/pci/hda/snd-hda-codec-generic.ko"
0xffffffffc03ed000-0xffffffffc03f6000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-rawmidi.ko"
0xffffffffc03fd000-0xffffffffc0401000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/hid/hid-generic.ko"
0xffffffffc0403000-0xffffffffc0407000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/drm_ttm_helper.ko"
0xffffffffc040d000-0xffffffffc0411000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/video/fbdev/core/sysfillrect.ko"
0xffffffffc0412000-0xffffffffc0416000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/video/fbdev/core/syscopyarea.ko"
0xffffffffc0417000-0xffffffffc041c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/misc/eeprom/ee1004.ko"
0xffffffffc041d000-0xffffffffc0421000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/mxm-wmi.ko"
0xffffffffc0423000-0xffffffffc0443000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/misc/mei/mei.ko"
0xffffffffc0449000-0xffffffffc044d000 None "/lib/modules/5.13.0-35-generic/kernel/crypto/crypto_simd.ko"
0xffffffffc0450000-0xffffffffc0454000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/video/fbdev/core/fb_sys_fops.ko"
0xffffffffc0455000-0xffffffffc0459000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/i2c/algos/i2c-algo-bit.ko"
0xffffffffc045a000-0xffffffffc045f000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/intel-wmi-thunderbolt.ko"
0xffffffffc0460000-0xffffffffc0477000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd.ko"
0xffffffffc047d000-0xffffffffc0481000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/crypto/ghash-clmulni-intel.ko"
0xffffffffc0483000-0xffffffffc0487000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/wmi-bmof.ko"
0xffffffffc0488000-0xffffffffc0496000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/media/rc/rc-core.ko"
0xffffffffc049e000-0xffffffffc04a2000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/platform/x86/eeepc-wmi.ko"
0xffffffffc04a6000-0xffffffffc04b0000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/misc/mei/mei-me.ko"
0xffffffffc04b1000-0xffffffffc04b8000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/input/joydev.ko"
0xffffffffc04ba000-0xffffffffc04bf000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/seq/snd-seq-midi.ko"
0xffffffffc04c0000-0xffffffffc04c4000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-hwdep.ko"
0xffffffffc04c6000-0xffffffffc04ca000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/firmware/efi/efi-pstore.ko"
0xffffffffc04cb000-0xffffffffc04d8000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/media/cec/core/cec.ko"
0xffffffffc04e1000-0xffffffffc04e5000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/xtensa/snd-sof-xtensa-dsp.ko"
0xffffffffc04e7000-0xffffffffc04f1000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-timer.ko"
0xffffffffc04f7000-0xffffffffc04fb000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/seq/snd-seq-midi-event.ko"
0xffffffffc04fc000-0xffffffffc0500000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/crypto/crct10dif-pclmul.ko"
0xffffffffc0501000-0xffffffffc0505000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/soundwire/soundwire-generic-allocation.ko"
0xffffffffc0507000-0xffffffffc050c000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/events/intel/intel-cstate.ko"
0xffffffffc050d000-0xffffffffc051c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/hid/usbhid/usbhid.ko"
0xffffffffc0531000-0xffffffffc053a000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/scheduler/gpu-sched.ko"
0xffffffffc053b000-0xffffffffc0540000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/hwmon/coretemp.ko"
0xffffffffc0542000-0xffffffffc0554000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/seq/snd-seq.ko"
0xffffffffc0562000-0xffffffffc0568000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/iommu/amd/iommu_v2.ko"
0xffffffffc0569000-0xffffffffc056d000 None "/lib/modules/5.13.0-35-generic/kernel/sound/ac97_bus.ko"
0xffffffffc056e000-0xffffffffc0573000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/thermal/intel/intel_powerclamp.ko"
0xffffffffc0575000-0xffffffffc0586000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/ttm/ttm.ko"
0xffffffffc0587000-0xffffffffc058e000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-compress.ko"
0xffffffffc0594000-0xffffffffc059a000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/powercap/intel_rapl_common.ko"
0xffffffffc059e000-0xffffffffc05a2000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/kernel/cpuid.ko"
0xffffffffc05a3000-0xffffffffc05a7000 None "/lib/modules/5.13.0-35-generic/kernel/lib/libcrc32c.ko"
0xffffffffc05a9000-0xffffffffc05ae000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/snd-sof-pci.ko"
0xffffffffc05b0000-0xffffffffc05b5000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/thermal/intel/x86_pkg_temp_thermal.ko"
0xffffffffc05b7000-0xffffffffc05f7000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/drm_kms_helper.ko"
0xffffffffc05f8000-0xffffffffc060c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/soundwire/soundwire-bus.ko"
0xffffffffc060d000-0xffffffffc0615000 None "/lib/modules/5.13.0-35-generic/kernel/sound/hda/ext/snd-hda-ext-core.ko"
0xffffffffc0617000-0xffffffffc0673000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/crypto/aesni-intel.ko"
0xffffffffc0677000-0xffffffffc0698000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/hid/hid.ko"
0xffffffffc06a6000-0xffffffffc06ab000 None "/lib/modules/5.13.0-35-generic/kernel/fs/fat/msdos.ko"
0xffffffffc06ac000-0xffffffffc06b2000 None "/lib/modules/5.13.0-35-generic/kernel/crypto/xor.ko"
0xffffffffc06be000-0xffffffffc06c3000 None "/lib/modules/5.13.0-35-generic/kernel/net/netfilter/nfnetlink.ko"
0xffffffffc06c4000-0xffffffffc06ce000 None "/lib/modules/5.13.0-35-generic/kernel/fs/minix/minix.ko"
0xffffffffc06d2000-0xffffffffc06ec000 None "/lib/modules/5.13.0-35-generic/kernel/fs/ntfs/ntfs.ko"
0xffffffffc0702000-0xffffffffc070e000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/intel/common/snd-soc-acpi-intel-match.ko"
0xffffffffc070f000-0xffffffffc0715000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/codecs/snd-soc-hdac-hda.ko"
0xffffffffc0721000-0xffffffffc0726000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/intel/snd-sof-intel-hda.ko"
0xffffffffc0727000-0xffffffffc072f000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/soundwire/soundwire-cadence.ko"
0xffffffffc0730000-0xffffffffc0734000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/intel/snd-sof-pci-intel-cnl.ko"
0xffffffffc0738000-0xffffffffc073c000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/thermal/intel/intel_tcc_cooling.ko"
0xffffffffc073d000-0xffffffffc0741000 None "/lib/modules/5.13.0-35-generic/kernel/fs/qnx4/qnx4.ko"
0xffffffffc0747000-0xffffffffc075e000 None "/lib/modules/5.13.0-35-generic/kernel/sound/hda/snd-hda-core.ko"
0xffffffffc075f000-0xffffffffc0763000 None "/lib/modules/5.13.0-35-generic/kernel/fs/nls/nls_iso8859-1.ko"
0xffffffffc0766000-0xffffffffc0783000 None "/lib/modules/5.13.0-35-generic/kernel/sound/core/snd-pcm.ko"
0xffffffffc0784000-0xffffffffc078b000 None "/lib/modules/5.13.0-35-generic/kernel/sound/hda/snd-intel-dspcfg.ko"
0xffffffffc0790000-0xffffffffc09e2000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/i915/i915.ko"
0xffffffffc09e3000-0xffffffffc0ab9000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/kvm/kvm.ko"
0xffffffffc0acf000-0xffffffffc0af3000 None "/lib/modules/5.13.0-35-generic/kernel/sound/pci/hda/snd-hda-codec-realtek.ko"
0xffffffffc0afa000-0xffffffffc0b09000 None "/lib/modules/5.13.0-35-generic/kernel/fs/hfs/hfs.ko"
0xffffffffc0b0c000-0xffffffffc0b12000 None "/lib/modules/5.13.0-35-generic/kernel/fs/binfmt_misc.ko"
0xffffffffc0b15000-0xffffffffc0b1a000 None "/lib/modules/5.13.0-35-generic/kernel/crypto/blake2b_generic.ko"
0xffffffffc0b23000-0xffffffffc0b44000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/snd-sof.ko"
0xffffffffc0b45000-0xffffffffc0b4f000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/soundwire/soundwire-intel.ko"
0xffffffffc0b5d000-0xffffffffc0ba5000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/snd-soc-core.ko"
0xffffffffc0ba6000-0xffffffffc0bc1000 None "/lib/modules/5.13.0-35-generic/kernel/fs/hfsplus/hfsplus.ko"
0xffffffffc0bc4000-0xffffffffc0bc9000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/powercap/intel_rapl_msr.ko"
0xffffffffc0bca000-0xffffffffc0be2000 None "/lib/modules/5.13.0-35-generic/kernel/sound/soc/sof/intel/snd-sof-intel-hda-common.ko"
0xffffffffc0be3000-0xffffffffc0bf7000 None "/lib/modules/5.13.0-35-generic/kernel/fs/ufs/ufs.ko"
0xffffffffc0c4c000-0xffffffffc0c59000 None "/lib/modules/5.13.0-35-generic/kernel/sound/pci/hda/snd-hda-intel.ko"
0xffffffffc0c68000-0xffffffffc0caf000 None "/lib/modules/5.13.0-35-generic/kernel/arch/x86/kvm/kvm-intel.ko"
0xffffffffc0cba000-0xffffffffc0e2f000 None "/lib/modules/5.13.0-35-generic/kernel/fs/xfs/xfs.ko"
0xffffffffc0e30000-0xffffffffc0e4c000 None "/lib/modules/5.13.0-35-generic/kernel/lib/raid6/raid6_pq.ko"
0xffffffffc0e4d000-0xffffffffc0e77000 None "/lib/modules/5.13.0-35-generic/kernel/lib/zstd/zstd_compress.ko"
0xffffffffc0e98000-0xffffffffc0ec7000 None "/lib/modules/5.13.0-35-generic/kernel/fs/jfs/jfs.ko"
0xffffffffc0fc6000-0xffffffffc0fea000 None "/lib/modules/5.13.0-35-generic/kernel/sound/pci/hda/snd-hda-codec.ko"
0xffffffffc0ff9000-0xffffffffc1008000 None "/lib/modules/5.13.0-35-generic/kernel/sound/pci/hda/snd-hda-codec-hdmi.ko"
0xffffffffc1038000-0xffffffffc1650000 None "/lib/modules/5.13.0-35-generic/kernel/drivers/gpu/drm/amd/amdgpu/amdgpu.ko"
0xffffffffc1651000-0xffffffffc17a9000 None "/lib/modules/5.13.0-35-generic/kernel/fs/btrfs/btrfs.ko"
Comm: {"pid": 542572, "tid": 542572, "name": "perf-exec"}
Comm: {"pid": 542572, "tid": 542572, "name": "dump_syms"}
0x000055ba9eb4d000-0x000055ba9f07e000 Some(CodeId(510d0a5c19eadf8043f203b4525be9be3dcb9554)) "/home/mstange/code/dump_syms/target/release/dump_syms"
0x00007f76b8720000-0x00007f76b8749000 None "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2"
0x00007ffdb48f7000-0x00007ffdb48f9000 Some(CodeId(0d82ee4bd7f9609c367095ba0bedf155b71cb058)) "[vdso]"
0x00007f76b84f6000-0x00007f76b8692000 None "/usr/lib/x86_64-linux-gnu/libstdc++.so.6.0.29"
0x00007f76b845e000-0x00007f76b84d0000 None "/usr/lib/x86_64-linux-gnu/libssl.so.1.1"
0x00007f76b8183000-0x00007f76b839e000 None "/usr/lib/x86_64-linux-gnu/libcrypto.so.1.1"
0x00007f76b8169000-0x00007f76b817e000 None "/usr/lib/x86_64-linux-gnu/libgcc_s.so.1"
0x00007f76b8085000-0x00007f76b810c000 None "/usr/lib/x86_64-linux-gnu/libm.so.6"
0x00007f76b7e5d000-0x00007f76b8019000 Some(CodeId(f0fc29165cbe6088c0e1adf03b0048fbecbc003a)) "/usr/lib/x86_64-linux-gnu/libc.so.6"
Have 23562 events, converted into 17801 processed samples.
Sample at t=1223104025585523 pid=542572 tid=542572
  arch_static_branch (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/jump_label.h:19)
  static_key_false (/build/linux-FwGwTr/linux-5.13.0/include/linux/jump_label.h:200)
  native_write_msr (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/msr.h:162)
  intel_pmu_enable_all (/build/linux-FwGwTr/linux-5.13.0/arch/x86/events/intel/core.c:2189)
  x86_pmu_enable (/build/linux-FwGwTr/linux-5.13.0/arch/x86/events/core.c:1349)
  ctx_resched (/build/linux-FwGwTr/linux-5.13.0/kernel/events/core.c:2755)
  pv_queued_spin_unlock (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/paravirt.h:590)
  queued_spin_unlock (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/qspinlock.h:56)
  do_raw_spin_unlock (/build/linux-FwGwTr/linux-5.13.0/include/linux/spinlock.h:212)
  _raw_spin_unlock (/build/linux-FwGwTr/linux-5.13.0/include/linux/spinlock_api_smp.h:151)
  perf_ctx_unlock (/build/linux-FwGwTr/linux-5.13.0/kernel/events/core.c:174)
  perf_event_enable_on_exec (/build/linux-FwGwTr/linux-5.13.0/kernel/events/core.c:4273)
  perf_event_exec (/build/linux-FwGwTr/linux-5.13.0/kernel/events/core.c:7677)
  begin_new_exec (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1356)
  load_elf_binary (/build/linux-FwGwTr/linux-5.13.0/fs/binfmt_elf.c:1002)
  search_binary_handler (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1724)
  exec_binprm (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1766)
  bprm_execve (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1834)
  bprm_execve (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1861)
  do_execveat_common (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:1923)
  _x64_sys_execve (/build/linux-FwGwTr/linux-5.13.0/fs/exec.c:2062)
  do_syscall_64 (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/common.c:47)
  <unknown> (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/entry_64.S:112)
  0x7f337451033b

[...]

Sample at t=1223104033727818 pid=542572 tid=542572
  <unknown> (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/idtentry.h:567)
  filemap_read (/build/linux-FwGwTr/linux-5.13.0/mm/filemap.c:2607)
  generic_file_read_iter (/build/linux-FwGwTr/linux-5.13.0/mm/filemap.c:2701)
  ext4_file_read_iter (/build/linux-FwGwTr/linux-5.13.0/fs/ext4/file.c:131)
  new_sync_read (/build/linux-FwGwTr/linux-5.13.0/fs/read_write.c:416)
  vfs_read (/build/linux-FwGwTr/linux-5.13.0/fs/read_write.c:496)
  ksys_read (/build/linux-FwGwTr/linux-5.13.0/fs/read_write.c:634)
  _x64_sys_read (/build/linux-FwGwTr/linux-5.13.0/fs/read_write.c:642)
  do_syscall_64 (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/common.c:47)
  <unknown> (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/entry_64.S:112)
  read
  std::sys::unix::fd::FileDesc::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys/unix/fd.rs:120)
  std::sys::unix::fs::File::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys/unix/fs.rs:870)
  <std::fs::File as std::io::Read>::read_buf (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/fs.rs:627)
  std::io::default_read_to_end (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/io/mod.rs:378)
  <std::fs::File as std::io::Read>::read_to_end (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/fs.rs:638)
  dump_syms::utils::read_file (/home/mstange/code/dump_syms/src/utils.rs:29)
  dump_syms::dumper::get_from_id (/home/mstange/code/dump_syms/src/dumper.rs:195)
  dump_syms::dumper::single_file (/home/mstange/code/dump_syms/src/dumper.rs:202)
  dump_syms::main (/home/mstange/code/dump_syms/src/main.rs:248)
  core::ops::function::FnOnce::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:227)
  std::sys_common::backtrace::__rust_begin_short_backtrace (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys_common/backtrace.rs:123)
  std::rt::lang_start::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:145)
  core::ops::function::impls::<impl core::ops::function::FnOnce<A> for &F>::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:259)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  main
  fun_29f50
  __libc_start_main
  _start
  0x7ffdb4824837

[...]

Sample at t=1223108807419100 pid=542572 tid=542572
  fun_a1b30
  fun_a3290
  fun_a4360
  realloc
  alloc::alloc::realloc (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:124)
  alloc::alloc::Global::grow_impl (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:201)
  <alloc::alloc::Global as core::alloc::Allocator>::grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:254)
  alloc::raw_vec::finish_grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:466)
  alloc::raw_vec::RawVec<T,A>::grow_amortized (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:402)
  alloc::raw_vec::RawVec<T,A>::reserve_for_push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:300)
  alloc::vec::Vec<T,A>::push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/vec/mod.rs:1726)
  cpp_demangle::ast::one_or_more (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:7700)
  <cpp_demangle::ast::BareFunctionType as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:4285)
  <cpp_demangle::ast::Encoding as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:1435)
  <cpp_demangle::ast::MangledName as cpp_demangle::ast::Parse>::parse (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/ast.rs:1341)
  cpp_demangle::Symbol<T>::new_with_options (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/cpp_demangle-0.3.3/src/lib.rs:238)
  symbolic_demangle::try_demangle_cpp (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-demangle-8.3.0/src/lib.rs:212)
  <symbolic_common::types::Name as symbolic_demangle::Demangle>::demangle (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-demangle-8.3.0/src/lib.rs:410)
  dump_syms::linux::elf::Collector::demangle (/home/mstange/code/dump_syms/src/linux/elf.rs:217)
  dump_syms::linux::elf::Collector::collect_function (/home/mstange/code/dump_syms/src/linux/elf.rs:281)
  dump_syms::linux::elf::Collector::collect_functions (/home/mstange/code/dump_syms/src/linux/elf.rs:308)
  dump_syms::linux::elf::ElfInfo::from_object (/home/mstange/code/dump_syms/src/linux/elf.rs:388)
  dump_syms::linux::elf::ElfInfo::new (/home/mstange/code/dump_syms/src/linux/elf.rs:368)
  <dump_syms::linux::elf::ElfInfo as dump_syms::dumper::Creator>::get_dbg (/home/mstange/code/dump_syms/src/dumper.rs:71)
  dump_syms::dumper::single_file (/home/mstange/code/dump_syms/src/dumper.rs:217)
  dump_syms::main (/home/mstange/code/dump_syms/src/main.rs:248)
  core::ops::function::FnOnce::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:227)
  std::sys_common::backtrace::__rust_begin_short_backtrace (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/sys_common/backtrace.rs:123)
  std::rt::lang_start::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:145)
  core::ops::function::impls::<impl core::ops::function::FnOnce<A> for &F>::call_once (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/core/src/ops/function.rs:259)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal::{{closure}} (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  std::panicking::try::do_call (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:406)
  std::panicking::try (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panicking.rs:370)
  std::panic::catch_unwind (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/panic.rs:133)
  std::rt::lang_start_internal (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/std/src/rt.rs:128)
  main
  fun_29f50
  __libc_start_main
  _start
  0x7ffdb4824837

[...]

Sample at t=1223117554603746 pid=542572 tid=542572
  fun_a3290
  fun_a4360
  realloc
  alloc::alloc::realloc (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:124)
  alloc::alloc::Global::grow_impl (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:201)
  <alloc::alloc::Global as core::alloc::Allocator>::grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/alloc.rs:254)
  alloc::raw_vec::finish_grow (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:466)
  alloc::raw_vec::RawVec<T,A>::grow_amortized (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:402)
  alloc::raw_vec::RawVec<T,A>::reserve_for_push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/raw_vec.rs:300)
  alloc::vec::Vec<T,A>::push (/rustc/9d1b2106e23b1abd32fce1f17267604a5102f57a/library/alloc/src/vec/mod.rs:1726)
  symbolic_minidump::cfi::AsciiCfiWriter<W>::process_fde (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-minidump-8.3.0/src/cfi.rs:613)
  symbolic_minidump::cfi::AsciiCfiWriter<W>::read_cfi (/home/mstange/.cargo/registry/src/github.com-1ecc6299db9ec823/symbolic-minidump-8.3.0/src/cfi.rs:580)
  <truncated stack>

[...]

Sample at t=1223121940171375 pid=542572 tid=542572
  _mod_zone_page_state (/build/linux-FwGwTr/linux-5.13.0/mm/vmstat.c:332)
  _mod_zone_freepage_state (/build/linux-FwGwTr/linux-5.13.0/include/linux/vmstat.h:408)
  _free_one_page (/build/linux-FwGwTr/linux-5.13.0/mm/page_alloc.c:1031)
  arch_static_branch (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/jump_label.h:19)
  static_key_false (/build/linux-FwGwTr/linux-5.13.0/include/linux/jump_label.h:200)
  trace_mm_page_pcpu_drain (/build/linux-FwGwTr/linux-5.13.0/include/trace/events/kmem.h:263)
  free_pcppages_bulk (/build/linux-FwGwTr/linux-5.13.0/mm/page_alloc.c:1464)
  free_unref_page_commit (/build/linux-FwGwTr/linux-5.13.0/mm/page_alloc.c:3280)
  free_unref_page_list (/build/linux-FwGwTr/linux-5.13.0/mm/page_alloc.c:3327)
  release_pages (/build/linux-FwGwTr/linux-5.13.0/mm/swap.c:973)
  free_pages_and_swap_cache (/build/linux-FwGwTr/linux-5.13.0/mm/swap_state.c:326)
  tlb_batch_pages_flush (/build/linux-FwGwTr/linux-5.13.0/mm/mmu_gather.c:50)
  tlb_flush_mmu_free (/build/linux-FwGwTr/linux-5.13.0/mm/mmu_gather.c:242)
  tlb_flush_mmu (/build/linux-FwGwTr/linux-5.13.0/mm/mmu_gather.c:249)
  tlb_finish_mmu (/build/linux-FwGwTr/linux-5.13.0/mm/mmu_gather.c:340)
  exit_mmap (/build/linux-FwGwTr/linux-5.13.0/mm/mmap.c:3217)
  _mmput (/build/linux-FwGwTr/linux-5.13.0/kernel/fork.c:1103)
  mmput (/build/linux-FwGwTr/linux-5.13.0/kernel/fork.c:1123)
  constant_test_bit (/build/linux-FwGwTr/linux-5.13.0/arch/x86/include/asm/bitops.h:207)
  test_bit (/build/linux-FwGwTr/linux-5.13.0/include/asm-generic/bitops/instrumented-non-atomic.h:135)
  test_ti_thread_flag (/build/linux-FwGwTr/linux-5.13.0/include/linux/thread_info.h:117)
  exit_mm (/build/linux-FwGwTr/linux-5.13.0/kernel/exit.c:502)
  do_exit (/build/linux-FwGwTr/linux-5.13.0/kernel/exit.c:815)
  signal_group_exit (/build/linux-FwGwTr/linux-5.13.0/include/linux/sched/signal.h:269)
  do_group_exit (/build/linux-FwGwTr/linux-5.13.0/kernel/exit.c:905)
  <unknown> (/build/linux-FwGwTr/linux-5.13.0/kernel/exit.c:933)
  do_syscall_64 (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/common.c:47)
  <unknown> (/build/linux-FwGwTr/linux-5.13.0/arch/x86/entry/entry_64.S:112)
  _Exit
```
