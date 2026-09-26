class SharedClass {
  constructor() {
    /** @type {number} */
    this.id;
  }
}
class SharedClass2 {
  constructor() {
    /** @type {number} */
    this.id = 1;
  }
}
const s = new SharedClass();
s.id;
const s2 = new SharedClass2();
s2.id;
