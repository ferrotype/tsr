export declare class UnknownDefault<T = unknown> { value: T; }
export declare class EmptyDefault<T = {}> { value: T; }
interface EmptyShape {}
export declare class StructuralDefault<T = EmptyShape> { value: T; }
export declare class StringDefault<T = string> { value: T; }
export declare class DependentDefault<T = unknown, U = T> { value: U; }
