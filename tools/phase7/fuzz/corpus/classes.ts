export interface Box<T> { value: T; }
export class Item implements Box<number> {
    constructor(public value: number) {}
}
export const item = new Item(1);
