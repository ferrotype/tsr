declare function cls<T extends abstract new (...args: any) => any>(target: T, context: ClassDecoratorContext<T>): void;
declare function method(target: (this: C) => number, context: ClassMethodDecoratorContext<C, (this: C) => number>): void;
declare function getter(target: (this: C) => number, context: ClassGetterDecoratorContext<C, number>): void;
declare function setter(target: (this: C, value: number) => void, context: ClassSetterDecoratorContext<C, number>): void;
declare function auto(target: ClassAccessorDecoratorTarget<C, number>, context: ClassAccessorDecoratorContext<C, number>): void;
declare function field(target: undefined, context: ClassFieldDecoratorContext<C, number>): void;
declare function param(target: object, key: string | symbol | undefined, index: number): void;
@cls
export class C {
  @method m() { return 1; }
  @getter get g() { return 1; }
  @setter set s(v: number) {}
  @auto accessor a = 1;
  @field f = 1;
  constructor(@param p: number) {}
}
