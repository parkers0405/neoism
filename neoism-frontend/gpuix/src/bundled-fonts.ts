import {readFileSync} from "node:fs";
import type {GpuixRenderer} from "@gpuix/native";
import cascadia from "../../../neoism-paint/src/font/resources/CascadiaCode/CascadiaCodeNF-Regular.otf" with {type:"file"};
import pixel from "../../../neoism-paint/src/font/resources/PressStart2P/PressStart2P-Regular.ttf" with {type:"file"};

/** Install fonts before native window creation, before any fallback is cached
 * and without waiting behind the first blank frame's presentation. */
export function prepareBundledFonts(renderer:GpuixRenderer){
 (renderer as GpuixRenderer & {setStartupFonts(fonts:Buffer[]):void}).setStartupFonts([cascadia,pixel].map(path=>readFileSync(path)));
}
