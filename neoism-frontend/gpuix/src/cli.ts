// Handle updates before importing graphics or starting a workspace daemon.
import {updateArguments,runUpdate} from "./update-cli";
import {docsArguments,runDocs} from "./local-docs/cli";
const update=updateArguments(process.argv.slice(2));
const docs=docsArguments(process.argv.slice(2));
if(docs){
 try{await runDocs(docs);}
 catch(error){console.error(`Neoism docs failed: ${error instanceof Error?error.message:String(error)}`);process.exitCode=1;}
}else if(update){
 try{process.exit(await runUpdate(update));}
 catch(error){console.error(`Neoism update failed: ${error instanceof Error?error.message:String(error)}`);process.exit(1);}
}else if(process.argv.includes("--neoism-verify-runtime")){
 await import("../scripts/verify-core");
}else{
 await import("./launch");
}
