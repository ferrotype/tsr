export {};
type Bad = intrinsic;
type BadGeneric<T> = intrinsic;
namespace Valid {
    export type BuiltinIteratorReturn = intrinsic;
    export type Uppercase<T> = intrinsic;
    export type Lowercase<T> = intrinsic;
    export type Capitalize<T> = intrinsic;
    export type Uncapitalize<T> = intrinsic;
    export type NoInfer<T> = intrinsic;
}
namespace Invalid {
    export type BuiltinIteratorReturn<T> = intrinsic;
    export type Uppercase = intrinsic;
    export type Lowercase<T, U> = intrinsic;
}
namespace Constraint {
    export type Uppercase<T extends Missing> = intrinsic;
}
const after: number = "still checked";
