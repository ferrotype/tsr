import { Service, Config } from "./metadata_types";
declare function inject(target: any, key: string | symbol | undefined, index: number): void;
declare function sealed(target: Function): void;
@sealed
export class Consumer {
  constructor(@inject service: Service, @inject config: Config) {}
}
