import {realpathSync} from "node:fs";
import {dlopen,FFIType,ptr} from "bun:ffi";

// Bun's Windows realpath preserves 8.3 aliases; Rust canonicalize returns the
// long verbatim path. Ask the OS for the same file identity/path on Windows.
const windows=process.platform==="win32"?dlopen("kernel32.dll",{
 CreateFileW:{args:[FFIType.ptr,FFIType.u32,FFIType.u32,FFIType.ptr,FFIType.u32,FFIType.u32,FFIType.ptr],returns:FFIType.ptr},
 GetFinalPathNameByHandleW:{args:[FFIType.ptr,FFIType.ptr,FFIType.u32,FFIType.u32],returns:FFIType.u32},
 CloseHandle:{args:[FFIType.ptr],returns:FFIType.i32},
}):undefined;
export function canonicalPath(path:string):string{
 if(!windows)return realpathSync(path);
 const name=Buffer.from(path+"\0","utf16le"),output=Buffer.alloc(65536);
 const handle=windows.symbols.CreateFileW(ptr(name),0,7,null,3,0x02000000,null);
 if(!handle||Number(handle)===-1||Number(handle)===2**64-1)throw Error(`Cannot open fixture path: ${path}`);
 try{
  const length=windows.symbols.GetFinalPathNameByHandleW(handle,ptr(output),output.length/2,0);
  if(!length||length>=output.length/2)throw Error(`Cannot canonicalize fixture path: ${path}`);
  return output.subarray(0,length*2).toString("utf16le");
 }finally{windows.symbols.CloseHandle(handle);}
}
