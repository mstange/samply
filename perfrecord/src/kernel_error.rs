use mach::kern_return::*;
use mach::message::*;
use thiserror::Error;

pub const KERN_INSUFFICIENT_BUFFER_SIZE: kern_return_t = 52;
pub const MACH_SEND_INVALID_CONTEXT: kern_return_t = 0x10000012;
pub const MACH_SEND_NO_GRANT_DEST: kern_return_t = 0x10000016;
pub const MACH_RCV_INVALID_REPLY: kern_return_t = 0x10004012;

pub trait IntoResult {
    type Value;
    type Error;

    fn into_result(self) -> std::result::Result<Self::Value, Self::Error>;
}

impl IntoResult for kern_return_t {
    type Value = ();
    type Error = KernelError;

    fn into_result(self) -> std::result::Result<(), KernelError> {
        if self == KERN_SUCCESS {
            Ok(())
        } else {
            Err(KernelError::from(self))
        }
    }
}

pub type Result<T> = std::result::Result<T, KernelError>;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum KernelError {
    #[error("Specified address is not currently valid.")]
    InvalidAddress,

    #[error("Specified memory is valid, but does not permit the required forms of access.")]
    ProtectionFailure,

    #[error("The address range specified is already in use, or no address range of the size specified could be found.")]
    NoSpace,

    #[error("The function requested was not applicable to this type of argument, or an argument is invalid")]
    InvalidArgument,

    #[error("The function could not be performed.  A catch-all.")]
    Failure,

    #[error("A system resource could not be allocated to fulfill this request.  This failure may not be permanent.")]
    ResourceShortage,

    #[error("The task in question does not hold receive rights for the port argument.")]
    NotReceiver,

    #[error("Bogus access restriction.")]
    NoAccess,

    #[error("During a page fault, the target address refers to a memory object that has been destroyed.  This failure is permanent.")]
    MemoryFailure,

    #[error("During a page fault, the memory object indicated that the data could not be returned.  This failure may be temporary; future attempts to access this same data may succeed, as defined by the memory object.")]
    MemoryError,

    #[error("The receive right is already a member of the portset.")]
    AlreadyInSet,

    #[error("The receive right is not a member of a port set.")]
    NotInSet,

    #[error("The name already denotes a right in the task.")]
    NameExists,

    #[error(
        "The operation was aborted.  Ipc code will catch this and reflect it as a message error."
    )]
    Aborted,

    #[error("The name doesn't denote a right in the task.")]
    InvalidName,

    #[error("Target task isn't an active task.")]
    InvalidTask,

    #[error("The name denotes a right, but not an appropriate right.")]
    InvalidRight,

    #[error("A blatant range error.")]
    InvalidValue,

    #[error("Operation would overflow limit on user-references.")]
    UrefsOverflow,

    #[error("The supplied (port) capability is improper.")]
    InvalidCapability,

    #[error("The task already has send or receive rights for the port under another name.")]
    RightExists,

    #[error("Target host isn't actually a host.")]
    InvalidHost,

    #[error("An attempt was made to supply \"precious\" data for memory that is already present in a memory object.")]
    MemoryPresent,

    #[error("A page was requested of a memory manager via memory_object_data_request for an object using a MEMORY_OBJECT_COPY_CALL strategy, with the VM_PROT_WANTS_COPY flag being used to specify that the page desired is for a copy of the object, and the memory manager has detected the page was pushed into a copy of the object while the kernel was walking the shadow chain from the copy to the object. This error code is delivered via memory_object_data_error and is handled by the kernel (it forces the kernel to restart the fault). It will not be seen by users.")]
    MemoryDataMoved,

    #[error("A strategic copy was attempted of an object upon which a quicker copy is now possible. The caller should retry the copy using vm_object_copy_quickly. This error code is seen only by the kernel.")]
    MemoryRestartCopy,

    #[error("An argument applied to assert processor set privilege was not a processor set control port.")]
    InvalidProcessorSet,

    #[error("The specified scheduling attributes exceed the thread's limits.")]
    PolicyLimit,

    #[error("The specified scheduling policy is not currently enabled for the processor set.")]
    InvalidPolicy,

    #[error("The external memory manager failed to initialize the memory object.")]
    InvalidObject,

    #[error(
        "A thread is attempting to wait for an event for which there is already a waiting thread."
    )]
    AlreadyWaiting,

    #[error("An attempt was made to destroy the default processor set.")]
    DefaultSet,

    #[error("An attempt was made to fetch an exception port that is protected, or to abort a thread while processing a protected exception.")]
    ExceptionProtected,

    #[error("A ledger was required but not supplied.")]
    InvalidLedger,

    #[error("The port was not a memory cache control port.")]
    InvalidMemoryControl,

    #[error("An argument supplied to assert security privilege was not a host security port.")]
    InvalidSecurity,

    #[error("thread_depress_abort was called on a thread which was not currently depressed.")]
    NotDepressed,

    #[error("Object has been terminated and is no longer available")]
    Terminated,

    #[error("Lock set has been destroyed and is no longer available.")]
    LockSetDestroyed,

    #[error("The thread holding the lock terminated before releasing the lock")]
    LockUnstable,

    #[error("The lock is already owned by another thread")]
    LockOwned,

    #[error("The lock is already owned by the calling thread")]
    LockOwnedSelf,

    #[error("Semaphore has been destroyed and is no longer available.")]
    SemaphoreDestroyed,

    #[error("Return from RPC indicating the target server was terminated before it successfully replied")]
    RpcServerTerminated,

    #[error("Terminate an orphaned activation.")]
    RpcTerminateOrphan,

    #[error("Allow an orphaned activation to continue executing.")]
    RpcContinueOrphan,

    #[error("Empty thread activation (No thread linked to it)")]
    NotSupported,

    #[error("Remote node down or inaccessible.")]
    NodeDown,

    #[error("A signalled thread was not actually waiting")]
    NotWaiting,

    #[error("Some thread-oriented operation (semaphore_wait) timed out")]
    OperationTimedOut,

    #[error("During a page fault, indicates that the page was rejected as a result of a signature check.")]
    CodesignError,

    #[error("The requested property cannot be changed at this time.")]
    PolicyStatic,

    #[error("The provided buffer is of insufficient size for the requested data.")]
    InsufficientBufferSize,

    #[error("Thread is waiting to send.  (Internal use only.)")]
    MachSendInProgress,

    #[error("Bogus in-line data.")]
    MachSendInvalidData,

    #[error("Bogus destination port.")]
    MachSendInvalidDest,

    #[error("Message not sent before timeout expired.")]
    MachSendTimedOut,

    #[error("Bogus voucher port.")]
    MachSendInvalidVoucher,

    #[error("Software interrupt.")]
    MachSendInterrupted,

    #[error("Data doesn't contain a complete message.")]
    MachSendMsgTooSmall,

    #[error("Bogus reply port.")]
    MachSendInvalidReply,

    #[error("Bogus port rights in the message body.")]
    MachSendInvalidRight,

    #[error("Bogus notify port argument.")]
    MachSendInvalidNotify,

    #[error("Invalid out-of-line memory pointer.")]
    MachSendInvalidMemory,

    #[error("No message buffer is available.")]
    MachSendNoBuffer,

    #[error("Send is too large for port")]
    MachSendTooLarge,

    #[error("Invalid msg-type specification.")]
    MachSendInvalidType,

    #[error("A field in the header had a bad value.")]
    MachSendInvalidHeader,

    #[error("The trailer to be sent does not match kernel format.")]
    MachSendInvalidTrailer,

    #[error("The sending thread context did not match the context on the dest port")]
    MachSendInvalidContext,

    #[error("compatibility: no longer a returned error")]
    MachSendInvalidRtOolSize,

    #[error("The destination port doesn't accept ports in body")]
    MachSendNoGrantDest,

    #[error("Thread is waiting for receive.  (Internal use only.)")]
    MachRcvInProgress,

    #[error("Bogus name for receive port/port-set.")]
    MachRcvInvalidName,

    #[error("Didn't get a message within the timeout value.")]
    MachRcvTimedOut,

    #[error("Message buffer is not large enough for inline data.")]
    MachRcvTooLarge,

    #[error("Software interrupt.")]
    MachRcvInterrupted,

    #[error("compatibility: no longer a returned error")]
    MachRcvPortChanged,

    #[error("Bogus notify port argument.")]
    MachRcvInvalidNotify,

    #[error("Bogus message buffer for inline data.")]
    MachRcvInvalidData,

    #[error("Port/set was sent away/died during receive.")]
    MachRcvPortDied,

    #[error("compatibility: no longer a returned error")]
    MachRcvInSet,

    #[error("Error receiving message header.  See special bits.")]
    MachRcvHeaderError,

    #[error("Error receiving message body.  See special bits.")]
    MachRcvBodyError,

    #[error("Invalid msg-type specification in scatter list.")]
    MachRcvInvalidType,

    #[error("Out-of-line overwrite region is not large enough")]
    MachRcvScatterSmall,

    #[error("trailer type or number of trailer elements not supported")]
    MachRcvInvalidTrailer,

    #[error("Waiting for receive with timeout. (Internal use only.)")]
    MachRcvInProgressTimed,

    #[error("invalid reply port used in a STRICT_REPLY message")]
    MachRcvInvalidReply,

    #[error("Unknown kernel error {0}")]
    Unknown(kern_return_t),
}

impl From<kern_return_t> for KernelError {
    fn from(err: kern_return_t) -> KernelError {
        match err {
            KERN_INVALID_ADDRESS => KernelError::InvalidAddress,
            KERN_PROTECTION_FAILURE => KernelError::ProtectionFailure,
            KERN_NO_SPACE => KernelError::NoSpace,
            KERN_INVALID_ARGUMENT => KernelError::InvalidArgument,
            KERN_FAILURE => KernelError::Failure,
            KERN_RESOURCE_SHORTAGE => KernelError::ResourceShortage,
            KERN_NOT_RECEIVER => KernelError::NotReceiver,
            KERN_NO_ACCESS => KernelError::NoAccess,
            KERN_MEMORY_FAILURE => KernelError::MemoryFailure,
            KERN_MEMORY_ERROR => KernelError::MemoryError,
            KERN_ALREADY_IN_SET => KernelError::AlreadyInSet,
            KERN_NOT_IN_SET => KernelError::NotInSet,
            KERN_NAME_EXISTS => KernelError::NameExists,
            KERN_ABORTED => KernelError::Aborted,
            KERN_INVALID_NAME => KernelError::InvalidName,
            KERN_INVALID_TASK => KernelError::InvalidTask,
            KERN_INVALID_RIGHT => KernelError::InvalidRight,
            KERN_INVALID_VALUE => KernelError::InvalidValue,
            KERN_UREFS_OVERFLOW => KernelError::UrefsOverflow,
            KERN_INVALID_CAPABILITY => KernelError::InvalidCapability,
            KERN_RIGHT_EXISTS => KernelError::RightExists,
            KERN_INVALID_HOST => KernelError::InvalidHost,
            KERN_MEMORY_PRESENT => KernelError::MemoryPresent,
            KERN_MEMORY_DATA_MOVED => KernelError::MemoryDataMoved,
            KERN_MEMORY_RESTART_COPY => KernelError::MemoryRestartCopy,
            KERN_INVALID_PROCESSOR_SET => KernelError::InvalidProcessorSet,
            KERN_POLICY_LIMIT => KernelError::PolicyLimit,
            KERN_INVALID_POLICY => KernelError::InvalidPolicy,
            KERN_INVALID_OBJECT => KernelError::InvalidObject,
            KERN_ALREADY_WAITING => KernelError::AlreadyWaiting,
            KERN_DEFAULT_SET => KernelError::DefaultSet,
            KERN_EXCEPTION_PROTECTED => KernelError::ExceptionProtected,
            KERN_INVALID_LEDGER => KernelError::InvalidLedger,
            KERN_INVALID_MEMORY_CONTROL => KernelError::InvalidMemoryControl,
            KERN_INVALID_SECURITY => KernelError::InvalidSecurity,
            KERN_NOT_DEPRESSED => KernelError::NotDepressed,
            KERN_TERMINATED => KernelError::Terminated,
            KERN_LOCK_SET_DESTROYED => KernelError::LockSetDestroyed,
            KERN_LOCK_UNSTABLE => KernelError::LockUnstable,
            KERN_LOCK_OWNED => KernelError::LockOwned,
            KERN_LOCK_OWNED_SELF => KernelError::LockOwnedSelf,
            KERN_SEMAPHORE_DESTROYED => KernelError::SemaphoreDestroyed,
            KERN_RPC_SERVER_TERMINATED => KernelError::RpcServerTerminated,
            KERN_RPC_TERMINATE_ORPHAN => KernelError::RpcTerminateOrphan,
            KERN_RPC_CONTINUE_ORPHAN => KernelError::RpcContinueOrphan,
            KERN_NOT_SUPPORTED => KernelError::NotSupported,
            KERN_NODE_DOWN => KernelError::NodeDown,
            KERN_NOT_WAITING => KernelError::NotWaiting,
            KERN_OPERATION_TIMED_OUT => KernelError::OperationTimedOut,
            KERN_CODESIGN_ERROR => KernelError::CodesignError,
            KERN_POLICY_STATIC => KernelError::PolicyStatic,
            KERN_INSUFFICIENT_BUFFER_SIZE => KernelError::InsufficientBufferSize,
            MACH_SEND_IN_PROGRESS => KernelError::MachSendInProgress,
            MACH_SEND_INVALID_DATA => KernelError::MachSendInvalidData,
            MACH_SEND_INVALID_DEST => KernelError::MachSendInvalidDest,
            MACH_SEND_TIMED_OUT => KernelError::MachSendTimedOut,
            MACH_SEND_INVALID_VOUCHER => KernelError::MachSendInvalidVoucher,
            MACH_SEND_INTERRUPTED => KernelError::MachSendInterrupted,
            MACH_SEND_MSG_TOO_SMALL => KernelError::MachSendMsgTooSmall,
            MACH_SEND_INVALID_REPLY => KernelError::MachSendInvalidReply,
            MACH_SEND_INVALID_RIGHT => KernelError::MachSendInvalidRight,
            MACH_SEND_INVALID_NOTIFY => KernelError::MachSendInvalidNotify,
            MACH_SEND_INVALID_MEMORY => KernelError::MachSendInvalidMemory,
            MACH_SEND_NO_BUFFER => KernelError::MachSendNoBuffer,
            MACH_SEND_TOO_LARGE => KernelError::MachSendTooLarge,
            MACH_SEND_INVALID_TYPE => KernelError::MachSendInvalidType,
            MACH_SEND_INVALID_HEADER => KernelError::MachSendInvalidHeader,
            MACH_SEND_INVALID_TRAILER => KernelError::MachSendInvalidTrailer,
            MACH_SEND_INVALID_CONTEXT => KernelError::MachSendInvalidContext,
            MACH_SEND_INVALID_RT_OOL_SIZE => KernelError::MachSendInvalidRtOolSize,
            MACH_SEND_NO_GRANT_DEST => KernelError::MachSendNoGrantDest,
            MACH_RCV_IN_PROGRESS => KernelError::MachRcvInProgress,
            MACH_RCV_INVALID_NAME => KernelError::MachRcvInvalidName,
            MACH_RCV_TIMED_OUT => KernelError::MachRcvTimedOut,
            MACH_RCV_TOO_LARGE => KernelError::MachRcvTooLarge,
            MACH_RCV_INTERRUPTED => KernelError::MachRcvInterrupted,
            MACH_RCV_PORT_CHANGED => KernelError::MachRcvPortChanged,
            MACH_RCV_INVALID_NOTIFY => KernelError::MachRcvInvalidNotify,
            MACH_RCV_INVALID_DATA => KernelError::MachRcvInvalidData,
            MACH_RCV_PORT_DIED => KernelError::MachRcvPortDied,
            MACH_RCV_IN_SET => KernelError::MachRcvInSet,
            MACH_RCV_HEADER_ERROR => KernelError::MachRcvHeaderError,
            MACH_RCV_BODY_ERROR => KernelError::MachRcvBodyError,
            MACH_RCV_INVALID_TYPE => KernelError::MachRcvInvalidType,
            MACH_RCV_SCATTER_SMALL => KernelError::MachRcvScatterSmall,
            MACH_RCV_INVALID_TRAILER => KernelError::MachRcvInvalidTrailer,
            MACH_RCV_IN_PROGRESS_TIMED => KernelError::MachRcvInProgressTimed,
            MACH_RCV_INVALID_REPLY => KernelError::MachRcvInvalidReply,
            unknown => KernelError::Unknown(unknown),
        }
    }
}
