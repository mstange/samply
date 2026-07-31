// Fixture for symbolication of code built with
// `-mllvm=-dwarf-linkage-names=Abstract`, which is what Firefox uses to keep its
// debug info small. With that flag, clang only puts DW_AT_linkage_name on
// *abstract* subprogram DIEs (the ones inlined frames point at). Concrete
// out-of-line definitions get none, so all the DWARF can tell us about them is
// their unqualified DW_AT_name, e.g. "Update" rather than
// "ns::Widget::Update(ns::Inner&, ns::Holder<int> const&) const".
//
// `-gsimple-template-names` is used for the same reason and makes the DWARF
// names even less complete: template arguments move into
// DW_TAG_template_type_parameter children instead of being spelled out in
// DW_AT_name.
//
// So this file needs, at a minimum:
//  - a namespaced, qualified, parameter-taking function which stays out of line,
//    to check that we recover its full name from the symbol table, and
//  - a function inlined into it, to check that inline frames still get their
//    names from the DWARF and are left alone.

namespace ns {

struct Inner {
  int value;
};

template <typename T>
class Holder {
 public:
  explicit Holder(T value) : mValue(value) {}
  T Get() const { return mValue; }

 private:
  T mValue;
};

class Widget {
 public:
  // Always inlined, so this only ever shows up as an inline frame. Its abstract
  // DIE keeps its linkage name.
  __attribute__((always_inline)) int Scale(int factor) const {
    return factor * 3 + mBias;
  }

  // Never inlined, so this is the outermost frame for any address inside it. Its
  // concrete DIE has no linkage name.
  __attribute__((noinline)) int Update(Inner& inner,
                                       const Holder<int>& holder) const;

 private:
  int mBias = 7;
};

__attribute__((noinline)) int Widget::Update(Inner& inner,
                                             const Holder<int>& holder) const {
  inner.value = Scale(holder.Get());
  return inner.value;
}

}  // namespace ns

int main(int argc, char** argv) {
  (void)argv;
  ns::Inner inner{argc};
  ns::Holder<int> holder(argc);
  ns::Widget widget;
  return widget.Update(inner, holder);
}
