import type {PaintCommand} from "./PaintLayer";
/** Move the complete original paint list, including clipping and glyph ink;
 * hit-testing is rebased by the same host offset. Raster pixels stay shared. */
export function translatePaintCommands(commands:readonly PaintCommand[],dx:number,dy:number):PaintCommand[]{
 const rect=(value:number[])=>[value[0]+dx,value[1]+dy,...value.slice(2)];
 const points=(value:number[][])=>value.map(point=>[point[0]+dx,point[1]+dy]);
 return commands.map(command=>{
  const clip="clip"in command&&command.clip?{clip:rect(command.clip)}:{};
  switch(command.kind){
   case "raster-text":case "rect":case "image":return{...command,...clip,rect:rect(command.rect)};
   case "text":return{...command,...clip,x:command.x+dx,y:command.y+dy};
   case "polygon":return{...command,points:points(command.points)};
   case "ring":return{...command,outer:points(command.outer),inner:points(command.inner)};
  }
 });
}
