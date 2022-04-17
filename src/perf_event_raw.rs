use crate::unaligned::{U16, U32, U64};
use zerocopy::FromBytes;

/// Structs and constants from perf_event.h

/// `perf_event_attr`
#[derive(FromBytes, Debug, Clone, Copy)]
#[repr(C)]
pub struct PerfEventAttr {
    /// Major type: hardware/software/tracepoint/etc.
    pub type_: U32,
    /// Size of the attr structure, for fwd/bwd compat.
    pub size: U32,
    /// Type-specific configuration information.
    pub config: U64,

    /// If ATTR_FLAG_BIT_FREQ is set in `flags`, this is the sample frequency,
    /// otherwise it is the sample period.
    ///
    /// ```c
    /// union {
    ///     /// Period of sampling
    ///     __u64 sample_period;
    ///     /// Frequency of sampling
    ///     __u64 sample_freq;
    /// };
    /// ```
    pub sampling_period_or_frequency: U64,

    /// Specifies values included in sample, see `perf_event_sample_format`.
    pub sample_type: U64,

    /// Specifies the structure values returned by read() on a perf event fd,
    /// see `perf_event_read_format`.
    pub read_format: U64,

    /// Bitset of ATTR_FLAG_BIT* flags
    pub flags: U64,

    /// If ATTR_FLAG_BIT_WATERMARK is set in `flags`, this is the watermark,
    /// otherwise it is the event count after which to wake up.
    ///
    /// ```c
    /// union {
    ///     /// wakeup every n events
    ///     __u32 wakeup_events;
    ///     /// bytes before wakeup
    ///     __u32 wakeup_watermark;
    /// };
    /// ```
    pub wakeup_events_or_watermark: U32,

    /// breakpoint type, uses HW_BREAKPOINT_* constants
    ///
    /// ```c
    /// HW_BREAKPOINT_EMPTY    = 0,
    /// HW_BREAKPOINT_R        = 1,
    /// HW_BREAKPOINT_W        = 2,
    /// HW_BREAKPOINT_RW       = HW_BREAKPOINT_R | HW_BREAKPOINT_W,
    /// HW_BREAKPOINT_X        = 4,
    /// HW_BREAKPOINT_INVALID  = HW_BREAKPOINT_RW | HW_BREAKPOINT_X,
    /// ```
    pub bp_type: U32,

    /// Union discriminator is ???
    ///
    /// ```c
    /// union {
    ///     __u64 bp_addr;
    ///     __u64 kprobe_func; /* for perf_kprobe */
    ///     __u64 uprobe_path; /* for perf_uprobe */
    ///     __u64 config1; /* extension of config */
    /// };
    /// ```
    pub bp_addr_or_kprobe_func_or_uprobe_func_or_config1: U64,

    /// Union discriminator is ???
    ///
    /// ```c
    /// union {
    ///     __u64 bp_len; /* breakpoint length, uses HW_BREAKPOINT_LEN_* constants */
    ///     __u64 kprobe_addr; /* when kprobe_func == NULL */
    ///     __u64 probe_offset; /* for perf_[k,u]probe */
    ///     __u64 config2; /* extension of config1 */
    /// };
    pub bp_len_or_kprobe_addr_or_probe_offset_or_config2: U64,

    /// Uses enum `perf_branch_sample_type`
    pub branch_sample_type: U64,

    /// Defines set of user regs to dump on samples.
    /// See asm/perf_regs.h for details.
    pub sample_regs_user: U64,

    /// Defines size of the user stack to dump on samples.
    pub sample_stack_user: U32,

    /// The clock ID.
    ///
    /// CLOCK_REALTIME = 0
    /// CLOCK_MONOTONIC = 1
    /// CLOCK_PROCESS_CPUTIME_ID = 2
    /// CLOCK_THREAD_CPUTIME_ID = 3
    /// CLOCK_MONOTONIC_RAW = 4
    /// CLOCK_REALTIME_COARSE = 5
    /// CLOCK_MONOTONIC_COARSE = 6
    /// CLOCK_BOOTTIME = 7
    /// CLOCK_REALTIME_ALARM = 8
    /// CLOCK_BOOTTIME_ALARM = 9
    pub clockid: U32,

    /// Defines set of regs to dump for each sample
    /// state captured on:
    ///  - precise = 0: PMU interrupt
    ///  - precise > 0: sampled instruction
    ///
    /// See asm/perf_regs.h for details.
    pub sample_regs_intr: U64,

    /// Wakeup watermark for AUX area
    pub aux_watermark: U32,

    /// When collecting stacks, this is the maximum number of stack frames
    /// (user + kernel) to collect.
    pub sample_max_stack: U16,

    pub __reserved_2: U16,

    /// When sampling AUX events, this is the size of the AUX sample.
    pub aux_sample_size: U32,

    pub __reserved_3: U32,

    /// User provided data if sigtrap=1, passed back to user via
    /// siginfo_t::si_perf_data, e.g. to permit user to identify the event.
    /// Note, siginfo_t::si_perf_data is long-sized, and sig_data will be
    /// truncated accordingly on 32 bit architectures.
    pub sig_data: U64,
}

/// off by default
pub const ATTR_FLAG_BIT_DISABLED: u64 = 1 << 0;
/// children inherit it
pub const ATTR_FLAG_BIT_INHERIT: u64 = 1 << 1;
/// must always be on PMU
pub const ATTR_FLAG_BIT_PINNED: u64 = 1 << 2;
/// only group on PMU
pub const ATTR_FLAG_BIT_EXCLUSIVE: u64 = 1 << 3;
/// don't count user
pub const ATTR_FLAG_BIT_EXCLUDE_USER: u64 = 1 << 4;
/// don't count kernel
pub const ATTR_FLAG_BIT_EXCLUDE_KERNEL: u64 = 1 << 5;
/// don't count hypervisor
pub const ATTR_FLAG_BIT_EXCLUDE_HV: u64 = 1 << 6;
/// don't count when idle
pub const ATTR_FLAG_BIT_EXCLUDE_IDLE: u64 = 1 << 7;
/// include mmap data
pub const ATTR_FLAG_BIT_MMAP: u64 = 1 << 8;
/// include comm data
pub const ATTR_FLAG_BIT_COMM: u64 = 1 << 9;
/// use freq, not period
pub const ATTR_FLAG_BIT_FREQ: u64 = 1 << 10;
/// per task counts
pub const ATTR_FLAG_BIT_INHERIT_STAT: u64 = 1 << 11;
/// next exec enables
pub const ATTR_FLAG_BIT_ENABLE_ON_EXEC: u64 = 1 << 12;
/// trace fork/exit
pub const ATTR_FLAG_BIT_TASK: u64 = 1 << 13;
/// wakeup_watermark
pub const ATTR_FLAG_BIT_WATERMARK: u64 = 1 << 14;
/// skid constraint
/// Specifies how precise the instruction address should be.
///
/// From the perf-list man page:
///
/// >     0 - SAMPLE_IP can have arbitrary skid
/// >     1 - SAMPLE_IP must have constant skid
/// >     2 - SAMPLE_IP requested to have 0 skid
/// >     3 - SAMPLE_IP must have 0 skid, or uses randomization to avoid
/// >         sample shadowing effects.
/// >
/// > For Intel systems precise event sampling is implemented with PEBS
/// > which supports up to precise-level 2, and precise level 3 for
/// > some special cases.
/// >
/// > On AMD systems it is implemented using IBS (up to precise-level
/// > 2). The precise modifier works with event types 0x76 (cpu-cycles,
/// > CPU clocks not halted) and 0xC1 (micro-ops retired). Both events
/// > map to IBS execution sampling (IBS op) with the IBS Op Counter
/// > Control bit (IbsOpCntCtl) set respectively (see AMD64
/// > Architecture Programmer’s Manual Volume 2: System Programming,
/// > 13.3 Instruction-Based Sampling). Examples to use IBS:
/// >
/// >     perf record -a -e cpu-cycles:p ...    # use ibs op counting cycles
/// >     perf record -a -e r076:p ...          # same as -e cpu-cycles:p
/// >     perf record -a -e r0C1:p ...          # use ibs op counting micro-ops
pub const ATTR_FLAG_BITMASK_PRECISE_IP: u64 = 1 << 15 | 1 << 16;
/// non-exec mmap data
pub const ATTR_FLAG_BIT_MMAP_DATA: u64 = 1 << 17;
/// sample_type all events
pub const ATTR_FLAG_BIT_SAMPLE_ID_ALL: u64 = 1 << 18;
/// don't count in host
pub const ATTR_FLAG_BIT_EXCLUDE_HOST: u64 = 1 << 19;
/// don't count in guest
pub const ATTR_FLAG_BIT_EXCLUDE_GUEST: u64 = 1 << 20;
/// exclude kernel callchains
pub const ATTR_FLAG_BIT_EXCLUDE_CALLCHAIN_KERNEL: u64 = 1 << 21;
/// exclude user callchains
pub const ATTR_FLAG_BIT_EXCLUDE_CALLCHAIN_USER: u64 = 1 << 22;
/// include mmap with inode data
pub const ATTR_FLAG_BIT_MMAP2: u64 = 1 << 23;
/// flag comm events that are due to exec
pub const ATTR_FLAG_BIT_COMM_EXEC: u64 = 1 << 24;
/// use @clockid for time fields
pub const ATTR_FLAG_BIT_USE_CLOCKID: u64 = 1 << 25;
/// context switch data
pub const ATTR_FLAG_BIT_CONTEXT_SWITCH: u64 = 1 << 26;
/// Write ring buffer from end to beginning
pub const ATTR_FLAG_BIT_WRITE_BACKWARD: u64 = 1 << 27;
/// include namespaces data
pub const ATTR_FLAG_BIT_NAMESPACES: u64 = 1 << 28;
/// include ksymbol events
pub const ATTR_FLAG_BIT_KSYMBOL: u64 = 1 << 29;
/// include bpf events
pub const ATTR_FLAG_BIT_BPF_EVENT: u64 = 1 << 30;
/// generate AUX records instead of events
pub const ATTR_FLAG_BIT_AUX_OUTPUT: u64 = 1 << 31;
/// include cgroup events
pub const ATTR_FLAG_BIT_CGROUP: u64 = 1 << 32;
/// include text poke events
pub const ATTR_FLAG_BIT_TEXT_POKE: u64 = 1 << 33;
/// use build id in mmap2 events
pub const ATTR_FLAG_BIT_BUILD_ID: u64 = 1 << 34;
/// children only inherit if cloned with CLONE_THREAD
pub const ATTR_FLAG_BIT_INHERIT_THREAD: u64 = 1 << 35;
/// event is removed from task on exec
pub const ATTR_FLAG_BIT_REMOVE_ON_EXEC: u64 = 1 << 36;
/// send synchronous SIGTRAP on event
pub const ATTR_FLAG_BIT_SIGTRAP: u64 = 1 << 37;

/*
 * If perf_event_attr.sample_id_all is set then all event types will
 * have the sample_type selected fields related to where/when
 * (identity) an event took place (TID, TIME, ID, STREAM_ID, CPU,
 * IDENTIFIER) described in PERF_RECORD_SAMPLE below, it will be stashed
 * just after the perf_event_header and the fields already present for
 * the existing fields, i.e. at the end of the payload. That way a newer
 * perf.data file will be supported by older perf tools, with these new
 * optional fields being ignored.
 *
 * struct sample_id {
 * 	{ u32			pid, tid; } && PERF_SAMPLE_TID
 * 	{ u64			time;     } && PERF_SAMPLE_TIME
 * 	{ u64			id;       } && PERF_SAMPLE_ID
 * 	{ u64			stream_id;} && PERF_SAMPLE_STREAM_ID
 * 	{ u32			cpu, res; } && PERF_SAMPLE_CPU
 *	{ u64			id;	  } && PERF_SAMPLE_IDENTIFIER
 * } && perf_event_attr::sample_id_all
 *
 * Note that PERF_SAMPLE_IDENTIFIER duplicates PERF_SAMPLE_ID.  The
 * advantage of PERF_SAMPLE_IDENTIFIER is that its position is fixed
 * relative to header.size.
 */

/*
 * The MMAP events record the PROT_EXEC mappings so that we can
 * correlate userspace IPs to code. They have the following structure:
 *
 * struct {
 *	struct perf_event_header	header;
 *
 *	u32				pid, tid;
 *	u64				addr;
 *	u64				len;
 *	u64				pgoff;
 *	char				filename[];
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_MMAP: u32 = 1;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u64				id;
 *	u64				lost;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_LOST: u32 = 2;

/*
 * struct {
 *	struct perf_event_header	header;
 *
 *	u32				pid, tid;
 *	char				comm[];
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_COMM: u32 = 3;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u32				pid, ppid;
 *	u32				tid, ptid;
 *	u64				time;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_EXIT: u32 = 4;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u64				time;
 *	u64				id;
 *	u64				stream_id;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_THROTTLE: u32 = 5;
pub const PERF_RECORD_UNTHROTTLE: u32 = 6;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u32				pid, ppid;
 *	u32				tid, ptid;
 *	u64				time;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_FORK: u32 = 7;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u32				pid, tid;
 *
 *	struct read_format		values;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_READ: u32 = 8;

/*
 * struct {
 *	struct perf_event_header	header;
 *
 *	#
 *	# Note that PERF_SAMPLE_IDENTIFIER duplicates PERF_SAMPLE_ID.
 *	# The advantage of PERF_SAMPLE_IDENTIFIER is that its position
 *	# is fixed relative to header.
 *	#
 *
 *	{ u64			id;	  } && PERF_SAMPLE_IDENTIFIER
 *	{ u64			ip;	  } && PERF_SAMPLE_IP
 *	{ u32			pid, tid; } && PERF_SAMPLE_TID
 *	{ u64			time;     } && PERF_SAMPLE_TIME
 *	{ u64			addr;     } && PERF_SAMPLE_ADDR
 *	{ u64			id;	  } && PERF_SAMPLE_ID
 *	{ u64			stream_id;} && PERF_SAMPLE_STREAM_ID
 *	{ u32			cpu, res; } && PERF_SAMPLE_CPU
 *	{ u64			period;   } && PERF_SAMPLE_PERIOD
 *
 *	{ struct read_format	values;	  } && PERF_SAMPLE_READ
 *
 *  #
 *  # The callchain includes both regular addresses, and special "context"
 *  # frames. The context frames are >= PERF_CONTEXT_MAX and annotate the
 *  # subsequent addresses as user / kernel / hypervisor / guest addresses.
 *  #
 *
 *	{ u64			nr,
 *	  u64			ips[nr];  } && PERF_SAMPLE_CALLCHAIN
 *
 *	#
 *	# The RAW record below is opaque data wrt the ABI
 *	#
 *	# That is, the ABI doesn't make any promises wrt to
 *	# the stability of its content, it may vary depending
 *	# on event, hardware, kernel version and phase of
 *	# the moon.
 *	#
 *	# In other words, PERF_SAMPLE_RAW contents are not an ABI.
 *	#
 *
 *	{ u32			size;
 *	  char                  data[size];}&& PERF_SAMPLE_RAW
 *
 *	{ u64                   nr;
 *	  { u64	hw_idx; } && PERF_SAMPLE_BRANCH_HW_INDEX
 *        { u64 from, to, flags } lbr[nr];
 *      } && PERF_SAMPLE_BRANCH_STACK
 *
 * 	{ u64			abi; # enum perf_sample_regs_abi
 * 	  u64			regs[weight(mask)]; } && PERF_SAMPLE_REGS_USER
 *
 * 	{ u64			size;
 * 	  char			data[size];
 * 	  u64			dyn_size; } && PERF_SAMPLE_STACK_USER
 *
 *	{ union perf_sample_weight
 *	 {
 *		u64		full; && PERF_SAMPLE_WEIGHT
 *	#if defined(__LITTLE_ENDIAN_BITFIELD)
 *		struct {
 *			u32	var1_dw;
 *			u16	var2_w;
 *			u16	var3_w;
 *		} && PERF_SAMPLE_WEIGHT_STRUCT
 *	#elif defined(__BIG_ENDIAN_BITFIELD)
 *		struct {
 *			u16	var3_w;
 *			u16	var2_w;
 *			u32	var1_dw;
 *		} && PERF_SAMPLE_WEIGHT_STRUCT
 *	#endif
 *	 }
 *	}
 *	{ u64			data_src; } && PERF_SAMPLE_DATA_SRC
 *	{ u64			transaction; } && PERF_SAMPLE_TRANSACTION
 *	{ u64			abi; # enum perf_sample_regs_abi
 *	  u64			regs[weight(mask)]; } && PERF_SAMPLE_REGS_INTR
 *	{ u64			phys_addr;} && PERF_SAMPLE_PHYS_ADDR
 *	{ u64			size;
 *	  char			data[size]; } && PERF_SAMPLE_AUX
 *	{ u64			data_page_size;} && PERF_SAMPLE_DATA_PAGE_SIZE
 *	{ u64			code_page_size;} && PERF_SAMPLE_CODE_PAGE_SIZE
 * };
 */
pub const PERF_RECORD_SAMPLE: u32 = 9;

/*
 * The MMAP2 records are an augmented version of MMAP, they add
 * maj, min, ino numbers to be used to uniquely identify each mapping
 *
 * struct {
 *	struct perf_event_header	header;
 *
 *	u32				pid, tid;
 *	u64				addr;
 *	u64				len;
 *	u64				pgoff;
 *	union {
 *		struct {
 *			u32		maj;
 *			u32		min;
 *			u64		ino;
 *			u64		ino_generation;
 *		};
 *		struct {
 *			u8		build_id_size;
 *			u8		__reserved_1;
 *			u16		__reserved_2;
 *			u8		build_id[20];
 *		};
 *	};
 *	u32				prot, flags;
 *	char				filename[];
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_MMAP2: u32 = 10;

/*
 * Records that new data landed in the AUX buffer part.
 *
 * struct {
 * 	struct perf_event_header	header;
 *
 * 	u64				aux_offset;
 * 	u64				aux_size;
 *	u64				flags;
 * 	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_AUX: u32 = 11;

/*
 * Indicates that instruction trace has started
 *
 * struct {
 *	struct perf_event_header	header;
 *	u32				pid;
 *	u32				tid;
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_ITRACE_START: u32 = 12;

/*
 * Records the dropped/lost sample number.
 *
 * struct {
 *	struct perf_event_header	header;
 *
 *	u64				lost;
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_LOST_SAMPLES: u32 = 13;

/*
 * Records a context switch in or out (flagged by
 * PERF_RECORD_MISC_SWITCH_OUT). See also
 * PERF_RECORD_SWITCH_CPU_WIDE.
 *
 * struct {
 *	struct perf_event_header	header;
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_SWITCH: u32 = 14;

/*
 * CPU-wide version of PERF_RECORD_SWITCH with next_prev_pid and
 * next_prev_tid that are the next (switching out) or previous
 * (switching in) pid/tid.
 *
 * struct {
 *	struct perf_event_header	header;
 *	u32				next_prev_pid;
 *	u32				next_prev_tid;
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_SWITCH_CPU_WIDE: u32 = 15;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u32				pid;
 *	u32				tid;
 *	u64				nr_namespaces;
 *	{ u64				dev, inode; } [nr_namespaces];
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_NAMESPACES: u32 = 16;

/*
 * Record ksymbol register/unregister events:
 *
 * struct {
 *	struct perf_event_header	header;
 *	u64				addr;
 *	u32				len;
 *	u16				ksym_type;
 *	u16				flags;
 *	char				name[];
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_KSYMBOL: u32 = 17;

/*
 * Record bpf events:
 *  enum perf_bpf_event_type {
 *	PERF_BPF_EVENT_UNKNOWN		= 0,
 *	PERF_BPF_EVENT_PROG_LOAD	= 1,
 *	PERF_BPF_EVENT_PROG_UNLOAD	= 2,
 *  };
 *
 * struct {
 *	struct perf_event_header	header;
 *	u16				type;
 *	u16				flags;
 *	u32				id;
 *	u8				tag[BPF_TAG_SIZE];
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_BPF_EVENT: u32 = 18;

/*
 * struct {
 *	struct perf_event_header	header;
 *	u64				id;
 *	char				path[];
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_CGROUP: u32 = 19;

/*
 * Records changes to kernel text i.e. self-modified code. 'old_len' is
 * the number of old bytes, 'new_len' is the number of new bytes. Either
 * 'old_len' or 'new_len' may be zero to indicate, for example, the
 * addition or removal of a trampoline. 'bytes' contains the old bytes
 * followed immediately by the new bytes.
 *
 * struct {
 *	struct perf_event_header	header;
 *	u64				addr;
 *	u16				old_len;
 *	u16				new_len;
 *	u8				bytes[];
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_TEXT_POKE: u32 = 20;

/*
 * Data written to the AUX area by hardware due to aux_output, may need
 * to be matched to the event by an architecture-specific hardware ID.
 * This records the hardware ID, but requires sample_id to provide the
 * event ID. e.g. Intel PT uses this record to disambiguate PEBS-via-PT
 * records from multiple events.
 *
 * struct {
 *	struct perf_event_header	header;
 *	u64				hw_id;
 *	struct sample_id		sample_id;
 * };
 */
pub const PERF_RECORD_AUX_OUTPUT_HW_ID: u32 = 21;

pub const PERF_SAMPLE_IP: u64 = 1 << 0;
pub const PERF_SAMPLE_TID: u64 = 1 << 1;
pub const PERF_SAMPLE_TIME: u64 = 1 << 2;
pub const PERF_SAMPLE_ADDR: u64 = 1 << 3;
pub const PERF_SAMPLE_READ: u64 = 1 << 4;
pub const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
pub const PERF_SAMPLE_ID: u64 = 1 << 6;
pub const PERF_SAMPLE_CPU: u64 = 1 << 7;
pub const PERF_SAMPLE_PERIOD: u64 = 1 << 8;
pub const PERF_SAMPLE_STREAM_ID: u64 = 1 << 9;
pub const PERF_SAMPLE_RAW: u64 = 1 << 10;
pub const PERF_SAMPLE_BRANCH_STACK: u64 = 1 << 11;
pub const PERF_SAMPLE_REGS_USER: u64 = 1 << 12;
pub const PERF_SAMPLE_STACK_USER: u64 = 1 << 13;
pub const PERF_SAMPLE_WEIGHT: u64 = 1 << 14;
pub const PERF_SAMPLE_DATA_SRC: u64 = 1 << 15;
pub const PERF_SAMPLE_IDENTIFIER: u64 = 1 << 16;
pub const PERF_SAMPLE_TRANSACTION: u64 = 1 << 17;
pub const PERF_SAMPLE_REGS_INTR: u64 = 1 << 18;
pub const PERF_SAMPLE_PHYS_ADDR: u64 = 1 << 19;
pub const PERF_SAMPLE_AUX: u64 = 1 << 20;
pub const PERF_SAMPLE_CGROUP: u64 = 1 << 21;
pub const PERF_SAMPLE_DATA_PAGE_SIZE: u64 = 1 << 22;
pub const PERF_SAMPLE_CODE_PAGE_SIZE: u64 = 1 << 23;
pub const PERF_SAMPLE_WEIGHT_STRUCT: u64 = 1 << 24;

pub const PERF_REG_X86_AX: u64 = 0;
pub const PERF_REG_X86_BX: u64 = 1;
pub const PERF_REG_X86_CX: u64 = 2;
pub const PERF_REG_X86_DX: u64 = 3;
pub const PERF_REG_X86_SI: u64 = 4;
pub const PERF_REG_X86_DI: u64 = 5;
pub const PERF_REG_X86_BP: u64 = 6;
pub const PERF_REG_X86_SP: u64 = 7;
pub const PERF_REG_X86_IP: u64 = 8;
pub const PERF_REG_X86_FLAGS: u64 = 9;
pub const PERF_REG_X86_CS: u64 = 10;
pub const PERF_REG_X86_SS: u64 = 11;
pub const PERF_REG_X86_DS: u64 = 12;
pub const PERF_REG_X86_ES: u64 = 13;
pub const PERF_REG_X86_FS: u64 = 14;
pub const PERF_REG_X86_GS: u64 = 15;
pub const PERF_REG_X86_R8: u64 = 16;
pub const PERF_REG_X86_R9: u64 = 17;
pub const PERF_REG_X86_R10: u64 = 18;
pub const PERF_REG_X86_R11: u64 = 19;
pub const PERF_REG_X86_R12: u64 = 20;
pub const PERF_REG_X86_R13: u64 = 21;
pub const PERF_REG_X86_R14: u64 = 22;
pub const PERF_REG_X86_R15: u64 = 23;

pub const PERF_REG_X86_32_MAX: u64 = PERF_REG_X86_GS + 1;
pub const PERF_REG_X86_64_MAX: u64 = PERF_REG_X86_R15 + 1;

pub const PERF_REG_ARM_R0: u64 = 0;
pub const PERF_REG_ARM_R1: u64 = 1;
pub const PERF_REG_ARM_R2: u64 = 2;
pub const PERF_REG_ARM_R3: u64 = 3;
pub const PERF_REG_ARM_R4: u64 = 4;
pub const PERF_REG_ARM_R5: u64 = 5;
pub const PERF_REG_ARM_R6: u64 = 6;
pub const PERF_REG_ARM_R7: u64 = 7;
pub const PERF_REG_ARM_R8: u64 = 8;
pub const PERF_REG_ARM_R9: u64 = 9;
pub const PERF_REG_ARM_R10: u64 = 10;
pub const PERF_REG_ARM_FP: u64 = 11;
pub const PERF_REG_ARM_IP: u64 = 12;
pub const PERF_REG_ARM_SP: u64 = 13;
pub const PERF_REG_ARM_LR: u64 = 14;
pub const PERF_REG_ARM_PC: u64 = 15;
pub const PERF_REG_ARM_MAX: u64 = 16;

pub const PERF_REG_MIPS_PC: u64 = 0;
pub const PERF_REG_MIPS_R1: u64 = 1;
pub const PERF_REG_MIPS_R2: u64 = 2;
pub const PERF_REG_MIPS_R3: u64 = 3;
pub const PERF_REG_MIPS_R4: u64 = 4;
pub const PERF_REG_MIPS_R5: u64 = 5;
pub const PERF_REG_MIPS_R6: u64 = 6;
pub const PERF_REG_MIPS_R7: u64 = 7;
pub const PERF_REG_MIPS_R8: u64 = 8;
pub const PERF_REG_MIPS_R9: u64 = 9;
pub const PERF_REG_MIPS_R10: u64 = 10;
pub const PERF_REG_MIPS_R11: u64 = 11;
pub const PERF_REG_MIPS_R12: u64 = 12;
pub const PERF_REG_MIPS_R13: u64 = 13;
pub const PERF_REG_MIPS_R14: u64 = 14;
pub const PERF_REG_MIPS_R15: u64 = 15;
pub const PERF_REG_MIPS_R16: u64 = 16;
pub const PERF_REG_MIPS_R17: u64 = 17;
pub const PERF_REG_MIPS_R18: u64 = 18;
pub const PERF_REG_MIPS_R19: u64 = 19;
pub const PERF_REG_MIPS_R20: u64 = 20;
pub const PERF_REG_MIPS_R21: u64 = 21;
pub const PERF_REG_MIPS_R22: u64 = 22;
pub const PERF_REG_MIPS_R23: u64 = 23;
pub const PERF_REG_MIPS_R24: u64 = 24;
pub const PERF_REG_MIPS_R25: u64 = 25;
pub const PERF_REG_MIPS_R28: u64 = 26;
pub const PERF_REG_MIPS_R29: u64 = 27;
pub const PERF_REG_MIPS_R30: u64 = 28;
pub const PERF_REG_MIPS_R31: u64 = 29;
pub const PERF_REG_MIPS_MAX: u64 = PERF_REG_MIPS_R31 + 1;

pub const PERF_REG_ARM64_X0: u64 = 0;
pub const PERF_REG_ARM64_X1: u64 = 1;
pub const PERF_REG_ARM64_X2: u64 = 2;
pub const PERF_REG_ARM64_X3: u64 = 3;
pub const PERF_REG_ARM64_X4: u64 = 4;
pub const PERF_REG_ARM64_X5: u64 = 5;
pub const PERF_REG_ARM64_X6: u64 = 6;
pub const PERF_REG_ARM64_X7: u64 = 7;
pub const PERF_REG_ARM64_X8: u64 = 8;
pub const PERF_REG_ARM64_X9: u64 = 9;
pub const PERF_REG_ARM64_X10: u64 = 10;
pub const PERF_REG_ARM64_X11: u64 = 11;
pub const PERF_REG_ARM64_X12: u64 = 12;
pub const PERF_REG_ARM64_X13: u64 = 13;
pub const PERF_REG_ARM64_X14: u64 = 14;
pub const PERF_REG_ARM64_X15: u64 = 15;
pub const PERF_REG_ARM64_X16: u64 = 16;
pub const PERF_REG_ARM64_X17: u64 = 17;
pub const PERF_REG_ARM64_X18: u64 = 18;
pub const PERF_REG_ARM64_X19: u64 = 19;
pub const PERF_REG_ARM64_X20: u64 = 20;
pub const PERF_REG_ARM64_X21: u64 = 21;
pub const PERF_REG_ARM64_X22: u64 = 22;
pub const PERF_REG_ARM64_X23: u64 = 23;
pub const PERF_REG_ARM64_X24: u64 = 24;
pub const PERF_REG_ARM64_X25: u64 = 25;
pub const PERF_REG_ARM64_X26: u64 = 26;
pub const PERF_REG_ARM64_X27: u64 = 27;
pub const PERF_REG_ARM64_X28: u64 = 28;
pub const PERF_REG_ARM64_X29: u64 = 29;
pub const PERF_REG_ARM64_LR: u64 = 30;
pub const PERF_REG_ARM64_SP: u64 = 31;
pub const PERF_REG_ARM64_PC: u64 = 32;
pub const PERF_REG_ARM64_MAX: u64 = 33;

pub const PERF_SAMPLE_REGS_ABI_32: u64 = 1;
pub const PERF_SAMPLE_REGS_ABI_64: u64 = 2;

pub const PERF_FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
pub const PERF_FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;
pub const PERF_FORMAT_ID: u64 = 1 << 2;
pub const PERF_FORMAT_GROUP: u64 = 1 << 3;

/*
 * values to program into branch_sample_type when PERF_SAMPLE_BRANCH is set
 *
 * If the user does not pass priv level information via branch_sample_type,
 * the kernel uses the event's priv level. Branch and event priv levels do
 * not have to match. Branch priv level is checked for permissions.
 *
 * The branch types can be combined, however BRANCH_ANY covers all types
 * of branches and therefore it supersedes all the other types.
 */
///  user branches
pub const PERF_SAMPLE_BRANCH_USER_SHIFT: u32 = 0;
///  kernel branches
pub const PERF_SAMPLE_BRANCH_KERNEL_SHIFT: u32 = 1;
///  hypervisor branches
pub const PERF_SAMPLE_BRANCH_HV_SHIFT: u32 = 2;
///  any branch types
pub const PERF_SAMPLE_BRANCH_ANY_SHIFT: u32 = 3;
///  any call branch
pub const PERF_SAMPLE_BRANCH_ANY_CALL_SHIFT: u32 = 4;
///  any return branch
pub const PERF_SAMPLE_BRANCH_ANY_RETURN_SHIFT: u32 = 5;
///  indirect calls
pub const PERF_SAMPLE_BRANCH_IND_CALL_SHIFT: u32 = 6;
///  transaction aborts
pub const PERF_SAMPLE_BRANCH_ABORT_TX_SHIFT: u32 = 7;
///  in transaction
pub const PERF_SAMPLE_BRANCH_IN_TX_SHIFT: u32 = 8;
///  not in transaction
pub const PERF_SAMPLE_BRANCH_NO_TX_SHIFT: u32 = 9;
///  conditional branches
pub const PERF_SAMPLE_BRANCH_COND_SHIFT: u32 = 10;
///  call/ret stack
pub const PERF_SAMPLE_BRANCH_CALL_STACK_SHIFT: u32 = 11;
///  indirect jumps
pub const PERF_SAMPLE_BRANCH_IND_JUMP_SHIFT: u32 = 12;
///  direct call
pub const PERF_SAMPLE_BRANCH_CALL_SHIFT: u32 = 13;
///  no flags
pub const PERF_SAMPLE_BRANCH_NO_FLAGS_SHIFT: u32 = 14;
///  no cycles
pub const PERF_SAMPLE_BRANCH_NO_CYCLES_SHIFT: u32 = 15;
///  save branch type
pub const PERF_SAMPLE_BRANCH_TYPE_SAVE_SHIFT: u32 = 16;
///  save low level index of raw branch records
pub const PERF_SAMPLE_BRANCH_HW_INDEX_SHIFT: u32 = 17;

pub const PERF_SAMPLE_BRANCH_USER: u64 = 1 << PERF_SAMPLE_BRANCH_USER_SHIFT;
pub const PERF_SAMPLE_BRANCH_KERNEL: u64 = 1 << PERF_SAMPLE_BRANCH_KERNEL_SHIFT;
pub const PERF_SAMPLE_BRANCH_HV: u64 = 1 << PERF_SAMPLE_BRANCH_HV_SHIFT;
pub const PERF_SAMPLE_BRANCH_ANY: u64 = 1 << PERF_SAMPLE_BRANCH_ANY_SHIFT;
pub const PERF_SAMPLE_BRANCH_ANY_CALL: u64 = 1 << PERF_SAMPLE_BRANCH_ANY_CALL_SHIFT;
pub const PERF_SAMPLE_BRANCH_ANY_RETURN: u64 = 1 << PERF_SAMPLE_BRANCH_ANY_RETURN_SHIFT;
pub const PERF_SAMPLE_BRANCH_IND_CALL: u64 = 1 << PERF_SAMPLE_BRANCH_IND_CALL_SHIFT;
pub const PERF_SAMPLE_BRANCH_ABORT_TX: u64 = 1 << PERF_SAMPLE_BRANCH_ABORT_TX_SHIFT;
pub const PERF_SAMPLE_BRANCH_IN_TX: u64 = 1 << PERF_SAMPLE_BRANCH_IN_TX_SHIFT;
pub const PERF_SAMPLE_BRANCH_NO_TX: u64 = 1 << PERF_SAMPLE_BRANCH_NO_TX_SHIFT;
pub const PERF_SAMPLE_BRANCH_COND: u64 = 1 << PERF_SAMPLE_BRANCH_COND_SHIFT;
pub const PERF_SAMPLE_BRANCH_CALL_STACK: u64 = 1 << PERF_SAMPLE_BRANCH_CALL_STACK_SHIFT;
pub const PERF_SAMPLE_BRANCH_IND_JUMP: u64 = 1 << PERF_SAMPLE_BRANCH_IND_JUMP_SHIFT;
pub const PERF_SAMPLE_BRANCH_CALL: u64 = 1 << PERF_SAMPLE_BRANCH_CALL_SHIFT;
pub const PERF_SAMPLE_BRANCH_NO_FLAGS: u64 = 1 << PERF_SAMPLE_BRANCH_NO_FLAGS_SHIFT;
pub const PERF_SAMPLE_BRANCH_NO_CYCLES: u64 = 1 << PERF_SAMPLE_BRANCH_NO_CYCLES_SHIFT;
pub const PERF_SAMPLE_BRANCH_TYPE_SAVE: u64 = 1 << PERF_SAMPLE_BRANCH_TYPE_SAVE_SHIFT;
pub const PERF_SAMPLE_BRANCH_HW_INDEX: u64 = 1 << PERF_SAMPLE_BRANCH_HW_INDEX_SHIFT;

// The current state of perf_event_header::misc bits usage:
// ('|' used bit, '-' unused bit)
//
//  012         CDEF
//  |||---------||||
//
//  Where:
//    0-2     CPUMODE_MASK
//
//    C       PROC_MAP_PARSE_TIMEOUT
//    D       MMAP_DATA / COMM_EXEC / FORK_EXEC / SWITCH_OUT
//    E       MMAP_BUILD_ID / EXACT_IP / SCHED_OUT_PREEMPT
//    F       (reserved)
pub const PERF_RECORD_MISC_CPUMODE_MASK: u16 = 0b111;
pub const PERF_RECORD_MISC_CPUMODE_UNKNOWN: u16 = 0;
pub const PERF_RECORD_MISC_KERNEL: u16 = 1;
pub const PERF_RECORD_MISC_USER: u16 = 2;
pub const PERF_RECORD_MISC_HYPERVISOR: u16 = 3;
pub const PERF_RECORD_MISC_GUEST_KERNEL: u16 = 4;
pub const PERF_RECORD_MISC_GUEST_USER: u16 = 5;
/// Indicates that /proc/PID/maps parsing are truncated by time out.
pub const PERF_RECORD_MISC_PROC_MAP_PARSE_TIMEOUT: u16 = 1 << 12;
// The following PERF_RECORD_MISC_* are used on different
// events, so can reuse the same bit position.
/// Used on PERF_RECORD_MMAP events to indicate mappings which are not executable.
/// Not used on PERF_RECORD_MMAP2 events - those have the full protection bitset.
pub const PERF_RECORD_MISC_MMAP_DATA: u16 = 1 << 13;
/// Used on PERF_RECORD_COMM event.
pub const PERF_RECORD_MISC_COMM_EXEC: u16 = 1 << 13;
/// Used on PERF_RECORD_FORK events (perf internal).
pub const PERF_RECORD_MISC_FORK_EXEC: u16 = 1 << 13;
/// Used on PERF_RECORD_SWITCH* events.
pub const PERF_RECORD_MISC_SWITCH_OUT: u16 = 1 << 13;
/// Indicates that the content of PERF_SAMPLE_IP points to
/// the actual instruction that triggered the event. See also
/// perf_event_attr::precise_ip.
/// Used on PERF_RECORD_SAMPLE of precise events.
pub const PERF_RECORD_MISC_EXACT_IP: u16 = 1 << 14;
/// Indicates that thread was preempted in TASK_RUNNING state.
/// Used on PERF_RECORD_SWITCH* events.
pub const PERF_RECORD_MISC_SWITCH_OUT_PREEMPT: u16 = 1 << 14;
/// Indicates that mmap2 event carries build id data.
/// Used on PERF_RECORD_MMAP2 events.
pub const PERF_RECORD_MISC_MMAP_BUILD_ID: u16 = 1 << 14;
/// Used in header.misc of the HEADER_BUILD_ID event. If set, the length
/// of the buildid is specified in the event (no more than 20).
pub const PERF_RECORD_MISC_BUILD_ID_SIZE: u16 = 1 << 15;

// These PERF_CONTEXT addresses are inserted into callchain to mark the
// "context" of the call chain addresses that follow. The special frames
// can be differentiated from real addresses by the fact that they are
// >= PERF_CONTEXT_MAX.
/// The callchain frames following this context marker frame are "hypervisor" frames.
pub const PERF_CONTEXT_HV: u64 = -32i64 as u64;
/// The callchain frames following this context marker frame are "kernel" frames.
pub const PERF_CONTEXT_KERNEL: u64 = -128i64 as u64;
/// The callchain frames following this context marker frame are "user" frames.
pub const PERF_CONTEXT_USER: u64 = -512i64 as u64;
/// The callchain frames following this context marker frame are "guest" frames.
pub const PERF_CONTEXT_GUEST: u64 = -2048i64 as u64;
/// The callchain frames following this context marker frame are "guest kernel" frames.
pub const PERF_CONTEXT_GUEST_KERNEL: u64 = -2176i64 as u64;
/// The callchain frames following this context marker frame are "guest user" frames.
pub const PERF_CONTEXT_GUEST_USER: u64 = -2560i64 as u64;
/// Any callchain frames which are >= PERF_CONTEXT_MAX are not real addresses;
/// instead, they mark the context of the subsequent callchain frames.
pub const PERF_CONTEXT_MAX: u64 = -4095i64 as u64;
