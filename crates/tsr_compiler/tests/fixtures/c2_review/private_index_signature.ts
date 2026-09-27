// A private property is not checked against the class's index signatures:
// getLiteralTypeFromProperty(prop, include, includeNonPublic = true) still
// yields "s", which is not applicable to the number index signature.
class C {
  [k: number]: number;
  private s = "x";
}
// The same property without the modifier is public and violates the string
// index signature below, which keeps the check itself observable.
class D {
  [k: string]: number;
  s = "x";
}
