# SPDX-License-Identifier: GPL-3.0-or-later
"""OS memory accounting and Windows child lifetime/commit limit.

ctypes is confined to documented Win32 process APIs; mesh/native algorithms
remain pure safe Rust. A Job Object handle lives only in Blender/the parent.
"""

import os

MIB = 1024 * 1024


def available_memory():
    if os.name == "nt":
        import ctypes as c
        from ctypes import wintypes as w

        class Status(c.Structure):
            _fields_ = [("length", w.DWORD), ("load", w.DWORD)] + [
                (name, c.c_ulonglong) for name in (
                    "physical", "available_physical", "pagefile", "available_pagefile",
                    "virtual", "available_virtual", "extended")]

        status = Status()
        status.length = c.sizeof(status)
        if c.windll.kernel32.GlobalMemoryStatusEx(c.byref(status)):
            return min(status.available_physical, status.available_pagefile)
        return None
    try:
        return os.sysconf("SC_AVPHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
    except (ValueError, OSError, AttributeError):
        return None


def memory_budget(limit_mb):
    if type(limit_mb) is not int or limit_mb < 256:
        raise ValueError("worker memory limit must be at least 256 MiB")
    budget = limit_mb * MIB
    available = available_memory()
    if available is not None:
        budget = min(budget, available // 2)
    if budget < 256 * MIB:
        raise MemoryError("not enough available memory to start a QuadLoom worker")
    return budget


def admit_host_memory(estimated):
    available = available_memory()
    if available is not None and estimated > available // 2:
        raise MemoryError("not enough available memory to copy/build this mesh; reduce its size")


class WindowsJob:
    """Limit the child's committed memory and kill it if the parent exits."""

    def __init__(self, process, budget):
        import ctypes as c
        from ctypes import wintypes as w

        class Basic(c.Structure):
            _fields_ = [("process_time", c.c_longlong), ("job_time", c.c_longlong),
                        ("flags", w.DWORD), ("min_working", c.c_size_t),
                        ("max_working", c.c_size_t), ("active", w.DWORD),
                        ("affinity", c.c_size_t), ("priority", w.DWORD), ("scheduling", w.DWORD)]

        class IO(c.Structure):
            _fields_ = [(name, c.c_ulonglong) for name in
                        ("read_ops", "write_ops", "other_ops", "read_bytes", "write_bytes", "other_bytes")]

        class Extended(c.Structure):
            _fields_ = [("basic", Basic), ("io", IO), ("process_memory", c.c_size_t),
                        ("job_memory", c.c_size_t), ("peak_process", c.c_size_t), ("peak_job", c.c_size_t)]

        kernel = c.WinDLL("kernel32", use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [c.c_void_p, w.LPCWSTR]
        kernel.CreateJobObjectW.restype = w.HANDLE
        kernel.SetInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD]
        kernel.SetInformationJobObject.restype = w.BOOL
        kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
        kernel.AssignProcessToJobObject.restype = w.BOOL
        kernel.CloseHandle.argtypes = [w.HANDLE]
        kernel.CloseHandle.restype = w.BOOL
        self.kernel, self.handle = kernel, kernel.CreateJobObjectW(None, None)
        if not self.handle:
            raise c.WinError(c.get_last_error())
        try:
            info = Extended()
            # PROCESS_MEMORY | KILL_ON_JOB_CLOSE | DIE_ON_UNHANDLED_EXCEPTION
            info.basic.flags = 0x100 | 0x2000 | 0x400
            info.process_memory = budget
            if not kernel.SetInformationJobObject(self.handle, 9, c.byref(info), c.sizeof(info)):
                raise c.WinError(c.get_last_error())
            if not kernel.AssignProcessToJobObject(self.handle, w.HANDLE(int(process._handle))):
                raise c.WinError(c.get_last_error())
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.handle:
            self.kernel.CloseHandle(self.handle)
            self.handle = None


def limit_child(budget):
    """Unix child-only limit; Windows uses the parent's Job Object."""
    if os.name != "nt":
        import resource
        resource.setrlimit(resource.RLIMIT_AS, (budget, budget))
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
