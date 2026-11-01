import "@gpuix/solid/jsx-runtime";
import type {SolidHostProps} from "@gpuix/solid/jsx-runtime";
declare module "@gpuix/solid/jsx-runtime" {
  namespace JSX {
    interface IntrinsicElements {
      device_viewport:SolidHostProps & {viewportId:number;frameRevision:number};
    }
  }
}