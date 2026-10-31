// Escaped Markdown inside JSON produces thousands of syntax boundaries on
// one source line. The padding also exercises the large-file highlight path.
export function wrappedJsonFixture(){
 return JSON.stringify({guideTemplate:("# Welcome\n\nUse \"quoted\" text, tabs\tand Unicode 界 👩‍💻. ").repeat(1200),
  padding:Array.from({length:2650},(_,i)=>`${i}: ${"padding ".repeat(30)}`)},null,2);
}
