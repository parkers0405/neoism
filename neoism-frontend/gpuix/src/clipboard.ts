// GPUIX currently exposes no native clipboard API. Keep OS clipboard access
// in the host; the shared Rust engines still own selection and register rules.
export function clipboardCommands(read: boolean,platform=process.platform,wayland=!!process.env.WAYLAND_DISPLAY): string[][] {
  if (platform === "darwin") return [[read ? "pbpaste" : "pbcopy"]];
  if (platform === "win32") return read
    ? [["powershell.exe", "-NoProfile", "-NonInteractive", "-STA", "-Command", "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::Write((Get-Clipboard -Raw))"]] : [["powershell.exe", "-NoProfile", "-NonInteractive", "-STA", "-Command", "Add-Type -AssemblyName System.Windows.Forms; [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [System.Windows.Forms.Clipboard]::SetText([Console]::In.ReadToEnd())"]];
  return wayland
    ? [[read ? "wl-paste" : "wl-copy", ...(read ? ["--no-newline"] : ["--type", "text/plain;charset=utf-8"])], ["xclip", "-selection", "clipboard", ...(read ? ["-o"] : [])]]
    : [["xclip", "-selection", "clipboard", ...(read ? ["-o"] : [])]];
}
export async function readClipboard(): Promise<string> {
  for (const command of clipboardCommands(true)) {
    const executable=Bun.which(command[0],{PATH:process.env.PATH});if(!executable)continue;
    const child = Bun.spawn([executable,...command.slice(1)], { stdout: "pipe", stderr: "ignore", windowsHide:true });
    const output = new Response(child.stdout).text();
    if (await child.exited === 0) return await output;
  }
  throw new Error("Clipboard is unavailable on this desktop.");
}
export async function writeClipboard(text: string): Promise<void> {
  for (const command of clipboardCommands(false)) {
    const executable=Bun.which(command[0],{PATH:process.env.PATH});if(!executable)continue;
    const child = Bun.spawn([executable,...command.slice(1)], { stdin: new TextEncoder().encode(text), stdout: "ignore", stderr: "ignore", windowsHide:true });
    if (await child.exited === 0) return;
  }
  throw new Error("Clipboard is unavailable on this desktop.");
}

/** PNG screenshot reads use the local desktop, even when connected to a remote server. */
export function clipboardImageCommands(platform=process.platform,wayland=!!process.env.WAYLAND_DISPLAY):Array<{command:string[];base64?:boolean}>{
  if(platform==="darwin")return [{base64:true,command:["osascript","-l","JavaScript","-e",`ObjC.import('AppKit');
const p=$.NSPasteboard.generalPasteboard,types=ObjC.deepUnwrap(p.types)||[];
let encoded='';
if(types.includes('public.png'))encoded=ObjC.unwrap(p.dataForType('public.png').base64EncodedStringWithOptions(0));
else {const type=['public.tiff','public.jpeg','com.compuserve.gif','com.microsoft.bmp'].find(type=>types.includes(type));if(type){const rep=$.NSBitmapImageRep.imageRepWithData(p.dataForType(type));const data=rep.representationUsingTypeProperties($.NSPNGFileType,$({}));encoded=ObjC.unwrap(data.base64EncodedStringWithOptions(0));}}
encoded;`]}];
  if(platform==="win32")return [{base64:true,command:["powershell.exe","-NoProfile","-NonInteractive","-STA","-Command",`Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; $image = [System.Windows.Forms.Clipboard]::GetImage(); if ($null -ne $image) { $stream = New-Object System.IO.MemoryStream; try { $image.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png); [Convert]::ToBase64String($stream.ToArray()) } finally { $stream.Dispose(); $image.Dispose() } }`]}];
  const x11={command:["xclip","-selection","clipboard","-t","image/png","-o"]};
  return wayland?[{command:["wl-paste","--no-newline","--type","image/png"]},x11]:[x11];
}
export async function readClipboardImage(maxBytes:number):Promise<Uint8Array|undefined>{
  for(const {command,base64} of clipboardImageCommands()){
    const executable=Bun.which(command[0],{PATH:process.env.PATH});if(!executable)continue;
    const child=Bun.spawn([executable,...command.slice(1)],{stdout:"pipe",stderr:"ignore",windowsHide:true});
    const timeout=setTimeout(()=>child.kill(),5000);
    try{
      const chunks:Uint8Array[]=[];let length=0;
      const reader=child.stdout.getReader();
      for(;;){const {done,value:chunk}=await reader.read();if(done)break;length+=chunk.length;if(length>Math.ceil(maxBytes*4/3)+1024){child.kill();throw new Error("Clipboard image exceeds the attachment size limit.");}chunks.push(chunk);}
      if(await child.exited!==0||!length)continue;
      const output=Buffer.concat(chunks),bytes=base64?Buffer.from(output.toString().trim(),"base64"):output;
      if(bytes.length>maxBytes)throw new Error("Clipboard image exceeds the attachment size limit.");
      if(bytes.length>=8&&bytes.subarray(0,8).equals(Buffer.from([137,80,78,71,13,10,26,10])))return bytes;
    }finally{clearTimeout(timeout);}
  }
}
