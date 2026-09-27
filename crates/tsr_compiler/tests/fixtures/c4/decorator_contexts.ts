function wrong(target: any, context: number) {}
export class C {
  @wrong static m() {}
  @wrong #p = 1;
  @wrong get g() { return 1; }
  @wrong accessor a = "";
}
@wrong export class D {}
function meta(target: any, context: ClassMethodDecoratorContext) {
  context.metadata.x = 1;
  const m: DecoratorMetadataObject = context.metadata;
}
export class E { @meta m() {} }
const md: DecoratorMetadata | null = E[Symbol.metadata];
const bad: number = E[Symbol.metadata];
