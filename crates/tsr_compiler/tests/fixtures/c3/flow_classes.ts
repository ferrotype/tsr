abstract class Base {
  abstract area(): number;
  describe(): string { return "base"; }
  get size(): number { return 1; }
  set size(value: number) {}
  #secret = 1;
  static { Base.count = 0; }
  static count: number;
  thisType(): this { return this; }
}
class Derived extends Base {
  override describe(): string { return "derived"; }
  area(): number { return 2; }
  extra(): void { this.#nope; }
}
class Wrong extends Base {
  describe(): string { return "missing override"; }
  override missing(): void {}
}
class NotAbstract extends Base {}
const b = new Base();
class PrivateNames {
  #a = 1;
  static peek(other: PrivateNames) { return other.#a; }
}
class Setters {
  get onlyGetter(): number { return 1; }
}
new Setters().onlyGetter = 2;
