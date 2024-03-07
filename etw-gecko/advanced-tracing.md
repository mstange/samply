### Circular Buffering
ETW supports recording into an in memory circular buffer. This will avoid
dropping events if the disk can't keep up. This is useful when profiling on low
performance machines under high load.
```
xperf -on  latency -stackwalk profile -Buffering -Buffersize 1024 -MinBuffers 50000 -MaxBuffers 50000
[do stuff you want to profile]
xperf -flush -f [output file]
xperf -stop
```

NOTE: converting circular buffer profiles is not yet supported.

### Unblocking stacks (Not yet suported)

```
xperf -on Latency+DISPATCHER -stackwalk Profile+CSwitch+ReadyThread
```


### Looking up providers/events

https://github.com/lallousx86/WinTools/tree/master/WEPExplorer is useful browser of this information

### Tracing with vsync
`xperf -start "NT Kernel Logger" -on latency -stackwalk profile -start "usersession" -on Microsoft-Windows-DxgKrnl:1:1`
`xperf -stop "NT Kernel Logger" -stop "usersession" -d out.etl`

### Tracing with Firefox events
```
xperf -start "NT Kernel Logger" -on latency -stackwalk profile -start "usersession" -on c923f508-96e4-5515-e32c-7539d1b10504
xperf -stop "NT Kernel Logger" -stop "usersession" -d out.etl
```

### Graphics tracing suggestions
```
xperf -start "NT Kernel Logger" -on latency -stackwalk profile -start "usersession" -on c923f508-96e4-5515-e32c-7539d1b10504 Microsoft-Windows-DxgKrnl:1:1 Microsoft-Windows-DirectComposition
xperf -stop "NT Kernel Logger" -stop "usersession" -d out.etl
```


### Stacks on page faults:
e.g. `xperf -on latency+ALL_FAULTS -stackwalk PagefaultDemandZero`
`latency` seems to be needed to get process information.
Add in calls to VirtualAlloc/VirtualFree
`xperf -on latency+ALL_FAULTS+VIRT_ALLOC -stackwalk PagefaultDemandZero`

### Stacks on syscalls:
`xperf -on syscall -stackwalk SyscallEnter` -- Use syscall branch

### Jscript
- Start Chrome with `chrome --js-flags="--enable-etw-stack-walking --interpreted-frames-native-stack"`
- `xperf -start "NT Kernel Logger" -on latency -stackwalk profile -start "usersession" -on Microsoft-JScript:0x3`
- `xperf -stop "NT Kernel Logger" -stop "usersession" -d out.etl`
