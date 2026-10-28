import {plugin} from "bun";
import {createRequire} from "node:module";
import {displayBindingLoader,verifiedDisplayBinding} from "../src/display-binding";
createRequire(import.meta.url)("./patch-solid-host.cjs")();
plugin({name:"neoism-native-display-binding",setup(build){
 build.onLoad({filter:/[/\\]@gpuix[/\\]native[/\\]index\.js$/},()=>{
  const {binding,stamp}=verifiedDisplayBinding();return {contents:displayBindingLoader(binding,stamp),loader:"js"};
 });
}});
