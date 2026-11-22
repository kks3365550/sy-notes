import ctypes
from ctypes import wintypes

DEBUG_PROCESS = 0x00000001
CREATE_UNICODE_ENVIRONMENT = 0x00000400

class STARTUPINFOW(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("lpReserved", wintypes.LPWSTR),
        ("lpDesktop", wintypes.LPWSTR),
        ("lpTitle", wintypes.LPWSTR),
        ("dwX", wintypes.DWORD),
        ("dwY", wintypes.DWORD),
        ("dwXSize", wintypes.DWORD),
        ("dwYSize", wintypes.DWORD),
        ("dwXCountChars", wintypes.DWORD),
        ("dwYCountChars", wintypes.DWORD),
        ("dwFillAttribute", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("wShowWindow", wintypes.WORD),
        ("cbReserved2", wintypes.WORD),
        ("lpReserved2", ctypes.c_void_p),
        ("hStdInput", wintypes.HANDLE),
        ("hStdOutput", wintypes.HANDLE),
        ("hStdError", wintypes.HANDLE),
    ]

class PROCESS_INFORMATION(ctypes.Structure):
    _fields_ = [
        ("hProcess", wintypes.HANDLE),
        ("hThread", wintypes.HANDLE),
        ("dwProcessId", wintypes.DWORD),
        ("dwThreadId", wintypes.DWORD),
    ]

class EXCEPTION_RECORD(ctypes.Structure):
    pass
EXCEPTION_RECORD._fields_ = [
    ("ExceptionCode", wintypes.DWORD),
    ("ExceptionFlags", wintypes.DWORD),
    ("ExceptionRecord", ctypes.POINTER(EXCEPTION_RECORD)),
    ("ExceptionAddress", ctypes.c_void_p),
    ("NumberParameters", wintypes.DWORD),
    ("ExceptionInformation", ctypes.c_void_p * 15),
]

class EXCEPTION_DEBUG_INFO(ctypes.Structure):
    _fields_ = [
        ("ExceptionRecord", EXCEPTION_RECORD),
        ("dwFirstChance", wintypes.DWORD),
    ]

class LOAD_DLL_DEBUG_INFO(ctypes.Structure):
    _fields_ = [
        ("hFile", wintypes.HANDLE),
        ("lpBaseOfDll", ctypes.c_void_p),
        ("dwDebugInfoFileOffset", wintypes.DWORD),
        ("nDebugInfoSize", wintypes.DWORD),
        ("lpImageName", ctypes.c_void_p),
        ("fUnicode", wintypes.WORD),
    ]

class OUTPUT_DEBUG_STRING_INFO(ctypes.Structure):
    _fields_ = [
        ("lpDebugStringData", ctypes.c_void_p),
        ("fUnicode", wintypes.WORD),
        ("nDebugStringLength", wintypes.WORD),
    ]

class UNION_U(ctypes.Union):
    _fields_ = [
        ("Exception", EXCEPTION_DEBUG_INFO),
        ("LoadDll", LOAD_DLL_DEBUG_INFO),
        ("DebugString", OUTPUT_DEBUG_STRING_INFO),
    ]

class DEBUG_EVENT(ctypes.Structure):
    _fields_ = [
        ("dwDebugEventCode", wintypes.DWORD),
        ("dwProcessId", wintypes.DWORD),
        ("dwThreadId", wintypes.DWORD),
        ("u", UNION_U),
    ]

kernel32 = ctypes.windll.kernel32

si = STARTUPINFOW()
si.cb = ctypes.sizeof(STARTUPINFOW)
pi = PROCESS_INFORMATION()

cmd = r'"C:\Users\kksjmj\Desktop\ai\sy-notes\src-tauri\target\debug\deps\concurrency_test-96868a29fc91001f.exe"'

ok = kernel32.CreateProcessW(
    None, cmd, None, None, False,
    DEBUG_PROCESS, None, r"C:\Users\kksjmj\Desktop\ai\sy-notes\src-tauri",
    ctypes.byref(si), ctypes.byref(pi)
)

if not ok:
    print("CreateProcessW failed:", kernel32.GetLastError())
    exit(1)

evt = DEBUG_EVENT()
while True:
    if not kernel32.WaitForDebugEvent(ctypes.byref(evt), 5000):
        print("Timeout waiting for debug event")
        break
    
    code = evt.dwDebugEventCode
    if code == 1: # EXCEPTION_DEBUG_EVENT
        exc_code = evt.u.Exception.ExceptionRecord.ExceptionCode
        print(f"EXCEPTION: {hex(exc_code)} at {hex(evt.u.Exception.ExceptionRecord.ExceptionAddress or 0)}")
        if exc_code == 0xc0000139:
            print("STATUS_ENTRYPOINT_NOT_FOUND encountered!")
            # The parameter often contains information or string pointer
            n_params = evt.u.Exception.ExceptionRecord.NumberParameters
            for i in range(n_params):
                val = evt.u.Exception.ExceptionRecord.ExceptionInformation[i]
                print(f"  Param[{i}] = {hex(val or 0)}")
    elif code == 6: # LOAD_DLL_DEBUG_EVENT
        # try to read image name
        print(f"LOAD DLL at {hex(evt.u.LoadDll.lpBaseOfDll or 0)}")
    elif code == 8: # OUTPUT_DEBUG_STRING_EVENT
        length = evt.u.DebugString.nDebugStringLength
        buf = ctypes.create_string_buffer(length)
        read = ctypes.c_size_t()
        kernel32.ReadProcessMemory(pi.hProcess, evt.u.DebugString.lpDebugStringData, buf, length, ctypes.byref(read))
        print("DEBUG STRING:", buf.value.decode('utf-8', errors='replace'))
    elif code == 5: # EXIT_PROCESS_DEBUG_EVENT
        print("EXIT PROCESS")
        kernel32.ContinueDebugEvent(evt.dwProcessId, evt.dwThreadId, 0x00010002)
        break

    kernel32.ContinueDebugEvent(evt.dwProcessId, evt.dwThreadId, 0x00010002)
