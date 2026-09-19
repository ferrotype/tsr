interface Hidden {}
namespace N { export interface Hidden {} }
export const variable: Hidden = null!;
export class Base {}
export class C<T extends Hidden> extends Base implements Hidden {
    static field: Hidden;
    field: Hidden;
    constructor(public parameter: Hidden) {}
    get accessor(): Hidden { return variable; }
    set accessor(value: Hidden) {}
    static get staticAccessor(): Hidden { return variable; }
    static set staticAccessor(value: Hidden) {}
    method<U extends Hidden>(parameter: Hidden): Hidden { return parameter; }
    static method<U extends Hidden>(parameter: Hidden): Hidden { return parameter; }
}
export class Private { private constructor(public parameter: Hidden) {} }
export interface I<T extends Hidden> {
    field: Hidden;
    method<U extends Hidden>(parameter: Hidden): Hidden;
    new<U extends Hidden>(parameter: Hidden): Hidden;
    (parameter: Hidden): Hidden;
    [key: string]: Hidden;
    get accessor(): Hidden;
    set accessor(value: Hidden);
}
export function exported<T extends Hidden>(parameter: Hidden): Hidden { return parameter; }
export type Alias<T extends Hidden> = Hidden;
export type Mapped<T> = {[P in keyof T]: T[P]};
export type Inferred<X> = X extends infer T ? T : never;
export type Fn = <T extends Hidden>(parameter: Hidden) => Hidden;
export type Ctor = new<T extends Hidden>(parameter: Hidden) => Hidden;
import Import = N.Hidden;
export const {field: binding = variable} = {field: variable};
C.field = variable;
C["field"] = variable;
Object.defineProperty(C, "defined", {get(): Hidden { return variable; }});
