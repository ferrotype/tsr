declare function any(...args: any[]): any;
export class S {
  static x = 1;
  @any static {
    S.x++;
  }
}
