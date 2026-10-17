"""GNU Windows tests need the same Common Controls manifest as the GUI binary."""
import ctypes
import pathlib
import sys

exe, manifest = map(pathlib.Path, sys.argv[1:])
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.BeginUpdateResourceW.argtypes = [ctypes.c_wchar_p, ctypes.c_bool]
kernel.BeginUpdateResourceW.restype = ctypes.c_void_p
kernel.UpdateResourceW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                                  ctypes.c_ushort, ctypes.c_void_p, ctypes.c_ulong]
kernel.UpdateResourceW.restype = ctypes.c_bool
kernel.EndUpdateResourceW.argtypes = [ctypes.c_void_p, ctypes.c_bool]
kernel.EndUpdateResourceW.restype = ctypes.c_bool
blob = manifest.read_bytes()
handle = kernel.BeginUpdateResourceW(str(exe.resolve()), False)
if not handle:
    raise ctypes.WinError(ctypes.get_last_error())
if not kernel.UpdateResourceW(handle, 24, 1, 1033, blob, len(blob)):
    code = ctypes.get_last_error()
    kernel.EndUpdateResourceW(handle, True)
    raise ctypes.WinError(code)
if not kernel.EndUpdateResourceW(handle, False):
    raise ctypes.WinError(ctypes.get_last_error())
