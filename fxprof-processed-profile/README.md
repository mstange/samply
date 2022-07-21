# fxprof-processed-profile

A crate that allows creating profiles in the [Firefox Profiler](https://github.com/firefox-devtools/profiler)'s ["Processed profile" format](https://github.com/firefox-devtools/profiler/blob/main/docs-developer/processed-profile-format.md).

Still work in progress, under-documented, and will have breaking changes frequently.

## Description

This crate is a sibling crate to the `gecko_profile` crate.

Profiles produced with this crate can be more efficient because they allow the Firefox Profiler to skip a processing step during loading, and because this format supports a "weight" column in the sample table. The sample weight can be used to collapse duplicate consecutive samples into one sample, which means that the individual sample timestamps don't have to be serialized into the JSON. This can save a ton of space.

## About the format

When the Firefox Profiler is used with Firefox, the Firefox Profiler receives profile data in the "Gecko profile" format. Then it converts it into the "processed profile" format.

The "processed profile" format is the format in which the files are stored when you upload the profile for sharing, or when you download it as a file. It is different from the "Gecko profile" format in the following ways:

 - There is one flat list of threads across all processes. Each thread comes with some information about its process, which allows the Firefox Profiler to group threads which belong to the same process.
 - Because of the flat list, the timestamps in all threads (from all processes) are relative to the same reference timestamp. This is different to the "Gecko profile" format where each process has its own time base.
 - The various tables in each thread are stored in a "struct of arrays" form. For example, the sample table has one flat list of timestamps, one flat list of stack indexes, and so forth. This is different to the "Gecko profile" format which contains one JS object for every individual sample - that object being an array such as `[stack_index, time, eventDelay, cpuDelta]`. The struct-of-arrays form makes the data cheaper to access and is much easier on the browser's GC.
 - The sample table in the "processed profile" format supports a weight column. The "Gecko profile" format currently does not have support for sample weights.
 - Each thread has a `funcTable`, a `resourceTable` and a `nativeSymbols` table. These tables do not exist in the "Gecko profile" format.
 - The structure of the `frameTable` is different. For example, each frame from the native stack has an integer code address, relative to the library that contains this address. In the "Gecko profile" format, the code address is stored in absolute form (process virtual memory address) as a hex string.
 - Native stacks in the "processed profile" format use "nudged" return addresses, i.e. return address minus one byte, so that they point into the calling instruction. This is different from the "Gecko profile" format, which uses raw return addresses.

The "processed profile" format is almost identical to the JavaScript object structure which the Firefox Profiler keeps in memory; [the only difference](https://github.com/firefox-devtools/profiler/blob/af469ed357890f816ab71fb4ba4c9fe125336d94/src/profile-logic/process-profile.js#L1539-L1556) being the use of `stringArray` (which is a plain JSON array of strings) instead of `stringTable` (which is an object containing both the array and a map for fast string-to-index lookup).
