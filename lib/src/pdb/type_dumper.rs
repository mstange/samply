use crate::pdb_crate::FallibleIterator;
use bitflags::bitflags;
use pdb::{
    ArgumentList, ArrayType, ClassKind, ClassType, FunctionAttributes, MemberFunctionType,
    ModifierType, PointerMode, PointerType, PrimitiveKind, PrimitiveType, ProcedureType, RawString,
    Result, TypeData, TypeFinder, TypeIndex, TypeInformation, UnionType, Variant,
};
use std::collections::HashMap;
use std::io::Write;

type FwdRefSize<'a> = HashMap<RawString<'a>, u32>;

#[derive(Eq, PartialEq)]
enum ThisKind {
    This,
    ConstThis,
    NotThis,
}

impl ThisKind {
    fn new(is_this: bool, is_const: bool) -> Self {
        if is_this {
            if is_const {
                Self::ConstThis
            } else {
                Self::This
            }
        } else {
            Self::NotThis
        }
    }
}

#[derive(Debug)]
struct PtrAttributes {
    is_pointer_const: bool,
    is_pointee_const: bool,
    mode: PointerMode,
}

bitflags! {
    pub struct DumperFlags: u32 {
        const NO_FUNCTION_RETURN = 0b1;
        const SPACE_AFTER_COMMA = 0b10;
        const SPACE_BEFORE_POINTER = 0b100;
        const NAME_ONLY = 0b1000;
    }
}

impl Default for DumperFlags {
    fn default() -> Self {
        Self::NO_FUNCTION_RETURN | Self::SPACE_AFTER_COMMA | Self::NAME_ONLY
    }
}

pub struct TypeDumper<'a> {
    finder: TypeFinder<'a>,
    fwd: FwdRefSize<'a>,
    ptr_size: u32,
    flags: DumperFlags,
}

pub enum ParentScope<'a> {
    WithType(TypeIndex),
    WithId(pdb::IdData<'a>),
}

impl<'a> TypeDumper<'a> {
    /// Collect all the Type and their TypeIndex to be able to search for a TypeIndex
    pub fn new<'b>(
        type_info: &'a TypeInformation<'b>,
        ptr_size: u32,
        flags: DumperFlags,
    ) -> Result<Self> {
        let mut types = type_info.iter();
        let mut finder = type_info.finder();

        // Some struct are incomplete so they've no size but they're forward references
        // So create a map containing names defining the struct (when they aren't fwd ref) and their size.
        // Once we'll need to compute a size for a fwd ref, we just use this map.
        let mut fwd = FwdRefSize::default();

        while let Some(typ) = types.next()? {
            finder.update(&types);
            if let Ok(typ) = typ.parse() {
                match typ {
                    TypeData::Class(t) => {
                        if !t.properties.forward_reference() {
                            let name = t.unique_name.unwrap_or(t.name);
                            fwd.insert(name, t.size.into());
                        }
                    }
                    TypeData::Union(t) => {
                        if !t.properties.forward_reference() {
                            let name = t.unique_name.unwrap_or(t.name);
                            fwd.insert(name, t.size);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            finder,
            fwd,
            ptr_size,
            flags,
        })
    }

    pub fn find(&self, index: TypeIndex) -> Result<TypeData> {
        let typ = self.finder.find(index).unwrap();
        typ.parse()
    }

    fn get_class_size(&self, typ: &ClassType) -> u32 {
        if typ.properties.forward_reference() {
            let name = typ.unique_name.unwrap_or(typ.name);

            // The name can not be in self.fwd because the type can be a forward reference to itself !!
            // (it's possible with an empty struct)
            *self.fwd.get(&name).unwrap_or(&typ.size.into())
        } else {
            typ.size.into()
        }
    }

    fn get_union_size(&self, typ: &UnionType) -> u32 {
        if typ.properties.forward_reference() {
            let name = typ.unique_name.unwrap_or(typ.name);
            *self.fwd.get(&name).unwrap_or(&typ.size)
        } else {
            typ.size
        }
    }

    pub fn get_type_size(&self, index: TypeIndex) -> u32 {
        let typ = self.find(index);
        typ.ok().map_or(0, |typ| self.get_data_size(&typ))
    }

    fn get_data_size(&self, typ: &TypeData) -> u32 {
        match typ {
            TypeData::Primitive(t) => {
                if t.indirection.is_some() {
                    return self.ptr_size;
                }
                match t.kind {
                    PrimitiveKind::NoType | PrimitiveKind::Void => 0,
                    PrimitiveKind::Char
                    | PrimitiveKind::UChar
                    | PrimitiveKind::RChar
                    | PrimitiveKind::I8
                    | PrimitiveKind::U8
                    | PrimitiveKind::Bool8 => 1,
                    PrimitiveKind::WChar
                    | PrimitiveKind::RChar16
                    | PrimitiveKind::Short
                    | PrimitiveKind::UShort
                    | PrimitiveKind::I16
                    | PrimitiveKind::U16
                    | PrimitiveKind::F16
                    | PrimitiveKind::Bool16 => 2,
                    PrimitiveKind::RChar32
                    | PrimitiveKind::Long
                    | PrimitiveKind::ULong
                    | PrimitiveKind::I32
                    | PrimitiveKind::U32
                    | PrimitiveKind::F32
                    | PrimitiveKind::F32PP
                    | PrimitiveKind::Bool32
                    | PrimitiveKind::HRESULT => 4,
                    PrimitiveKind::I64
                    | PrimitiveKind::U64
                    | PrimitiveKind::Quad
                    | PrimitiveKind::UQuad
                    | PrimitiveKind::F64
                    | PrimitiveKind::Complex32
                    | PrimitiveKind::Bool64 => 8,
                    PrimitiveKind::I128
                    | PrimitiveKind::U128
                    | PrimitiveKind::Octa
                    | PrimitiveKind::UOcta
                    | PrimitiveKind::F128
                    | PrimitiveKind::Complex64 => 16,
                    PrimitiveKind::F48 => 6,
                    PrimitiveKind::F80 => 10,
                    PrimitiveKind::Complex80 => 20,
                    PrimitiveKind::Complex128 => 32,
                }
            }
            TypeData::Class(t) => self.get_class_size(t),
            TypeData::MemberFunction(_) => self.ptr_size,
            TypeData::Procedure(_) => self.ptr_size,
            TypeData::Pointer(t) => t.attributes.size().into(),
            TypeData::Array(t) => *t.dimensions.last().unwrap(),
            TypeData::Union(t) => self.get_union_size(t),
            TypeData::Enumeration(t) => self.get_type_size(t.underlying_type),
            TypeData::Enumerate(t) => match t.value {
                Variant::I8(_) | Variant::U8(_) => 1,
                Variant::I16(_) | Variant::U16(_) => 2,
                Variant::I32(_) | Variant::U32(_) => 4,
                Variant::I64(_) | Variant::U64(_) => 8,
            },
            TypeData::Modifier(t) => self.get_type_size(t.underlying_type),
            _ => 0,
        }
    }

    fn dump_parent_scope(&self, w: &mut impl Write, scope: ParentScope) -> Result<()> {
        match scope {
            ParentScope::WithType(scope_index) => match self.find(scope_index)? {
                TypeData::Class(c) => write!(w, "{}::", c.name)?,
                other => write!(w, "<unhandled scope type {:?}>::", other)?,
            },
            ParentScope::WithId(id_data) => match id_data {
                pdb::IdData::String(s) => write!(w, "{}::", s.name)?,
                other => write!(w, "<unhandled id scope {:?}>::", other)?,
            },
        }
        Ok(())
    }

    /// Dump a ProcedureType at the given TypeIndex
    /// If the TypeIndex is 0 then try to use demanglers to have the correct name
    pub fn dump_function(
        &self,
        name: &str,
        index: TypeIndex,
        parent_index: Option<ParentScope>,
    ) -> Result<String> {
        if name.is_empty() {
            Ok("<name omitted>".to_string())
        } else if index == TypeIndex(0) {
            Ok(name.to_string())
        } else {
            let mut w: Vec<u8> = Vec::new();
            let typ = self.find(index)?;
            match typ {
                TypeData::MemberFunction(t) => {
                    let ztatic = t.this_pointer_type.is_none();
                    if ztatic {
                        w.write_all(b"static ")?;
                    }
                    let no_return = self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN);
                    let ret = self.get_return_type(Some(t.return_type), t.attributes, no_return);
                    Self::dump_return(&mut w, ret)?;
                    let (const_meth, args) = self.dump_method_parts(t, ztatic)?;
                    if let Some(i) = parent_index {
                        self.dump_parent_scope(&mut w, i)?;
                    }
                    write!(w, "{}", name)?;
                    write!(w, "({})", args)?;
                    if const_meth {
                        w.write_all(b" const")?;
                    }
                }
                TypeData::Procedure(t) => {
                    let no_return = self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN);
                    let ret = self.get_return_type(t.return_type, t.attributes, no_return);
                    Self::dump_return(&mut w, ret)?;
                    if let Some(i) = parent_index {
                        self.dump_parent_scope(&mut w, i)?;
                    }
                    write!(w, "{}", name)?;
                    let args = self.dump_index(t.argument_list)?;
                    write!(w, "({})", args)?;
                }
                _ => return Ok(name.to_string()),
            }
            Ok(String::from_utf8(w)
                .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).to_string()))
        }
    }

    #[inline(always)]
    fn dump_return(w: &mut impl Write, name: String) -> Result<()> {
        if !name.is_empty() {
            write!(w, "{} ", name)?;
        }
        Ok(())
    }

    fn get_return_type(
        &self,
        typ: Option<TypeIndex>,
        attrs: FunctionAttributes,
        no_return: bool,
    ) -> String {
        typ.filter(|_| !no_return && !attrs.is_constructor())
            .and_then(|r| self.dump_index(r).ok())
            .map_or_else(|| "".to_string(), |r| r)
    }

    fn check_this_type(&self, this: TypeIndex, class: TypeIndex) -> Result<ThisKind> {
        let this = self.find(this)?;

        let is_this = match this {
            TypeData::Pointer(ptr) => {
                if ptr.underlying_type == class {
                    ThisKind::This
                } else {
                    let underlying_typ = self.find(ptr.underlying_type)?;
                    if let TypeData::Modifier(modifier) = underlying_typ {
                        ThisKind::new(modifier.underlying_type == class, modifier.constant)
                    } else {
                        ThisKind::NotThis
                    }
                }
            }
            TypeData::Modifier(modifier) => {
                let underlying_typ = self.find(modifier.underlying_type)?;
                if let TypeData::Pointer(ptr) = underlying_typ {
                    ThisKind::new(ptr.underlying_type == class, modifier.constant)
                } else {
                    ThisKind::NotThis
                }
            }
            _ => ThisKind::NotThis,
        };
        Ok(is_this)
    }

    fn dump_method_parts(&self, typ: MemberFunctionType, ztatic: bool) -> Result<(bool, String)> {
        let args_typ = self.dump_index(typ.argument_list)?;
        // Note: "this" isn't dumped but there are some cases in rust code where
        // a first argument shouldn't be "this" but in fact it is:
        // https://hg.mozilla.org/releases/mozilla-release/annotate/7ece03f6971968eede29275477502309bbe399da/toolkit/components/bitsdownload/src/bits_interface/task/service_task.rs#l217
        // So we dump "this" when the underlying type (modulo pointer) is different from the class type

        let (args_typ, const_meth) = if !ztatic {
            let this_typ = typ.this_pointer_type.unwrap();
            let this_kind = self.check_this_type(this_typ, typ.class_type)?;
            if this_kind == ThisKind::NotThis {
                let this_typ = self.dump_index(this_typ)?;
                if args_typ.is_empty() {
                    (this_typ, false)
                } else {
                    (format!("{}, {}", this_typ, args_typ), false)
                }
            } else {
                (args_typ, this_kind == ThisKind::ConstThis)
            }
        } else {
            (args_typ, false)
        };

        Ok((const_meth, args_typ))
    }

    fn dump_attributes(&self, attrs: Vec<PtrAttributes>) -> String {
        attrs
            .iter()
            .rev()
            .fold(String::new(), |mut buf, attr| {
                if attr.is_pointee_const {
                    if self.flags.intersects(DumperFlags::SPACE_BEFORE_POINTER) {
                        buf.push_str(" const ");
                    } else {
                        buf.push_str(" const");
                    }
                }
                match attr.mode {
                    PointerMode::Pointer => buf.push('*'),
                    PointerMode::LValueReference => buf.push('&'),
                    PointerMode::Member => buf.push_str("::*"),
                    PointerMode::MemberFunction => buf.push_str("::*"),
                    PointerMode::RValueReference => buf.push_str("&&"),
                }
                if attr.is_pointer_const {
                    if self.flags.intersects(DumperFlags::SPACE_BEFORE_POINTER) {
                        buf.push_str(" const ");
                    } else {
                        buf.push_str(" const");
                    }
                }
                buf
            })
            .trim()
            .to_string()
    }

    fn dump_member_ptr(
        &self,
        fun: MemberFunctionType,
        attributes: Vec<PtrAttributes>,
    ) -> Result<String> {
        let ztatic = fun.this_pointer_type.is_none();
        let mut w: Vec<u8> = Vec::new();
        let ret = self.get_return_type(Some(fun.return_type), fun.attributes, false);
        Self::dump_return(&mut w, ret)?;
        let (_, args) = self.dump_method_parts(fun, ztatic)?;
        let class = self.dump_index(fun.class_type)?;
        write!(w, "({}", class)?;
        let attrs = self.dump_attributes(attributes);
        write!(w, "{})", attrs)?;
        write!(w, "({})", args)?;
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_proc_ptr(&self, fun: ProcedureType, attributes: Vec<PtrAttributes>) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        let no_return = false;
        let ret = self.get_return_type(fun.return_type, fun.attributes, no_return);
        Self::dump_return(&mut w, ret)?;
        let attrs = self.dump_attributes(attributes);
        write!(w, "({})", attrs)?;
        let args = self.dump_index(fun.argument_list)?;
        write!(w, "({})", args)?;
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_other_ptr(&self, typ: TypeData, attributes: Vec<PtrAttributes>) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        // Output: <typ> <attrs>, possibly with a space in between.
        let typ = self.dump_data(typ)?;
        let attrs = self.dump_attributes(attributes);

        // Do we need a space between typ and attrs?
        let need_space = if attrs.starts_with('c') {
            // The first attribute has a const pointee, so the attributes start with
            // "const &&" or "const&&", for example. Always insert a space before const.
            true
        } else if self.flags.intersects(DumperFlags::SPACE_BEFORE_POINTER) {
            let c = typ.chars().last().unwrap();
            let type_is_pointer = c == '*' || c == '&';
            if type_is_pointer {
                // The type is a pointer, and we put the space before the
                // pointer. So there is already a space just in front of the
                // pointer sigil, and we can skip the space after the sigil.
                false
            } else {
                // The type does not end in a pointer sigil. Have a space.
                true
            }
        } else {
            // No space before pointer, that means space after pointer.
            // If the type is a pointer, it will already come with a space just
            // before its pointer sigil.
            // TODO: What if the type is not a pointer?
            false
        };
        let space = if need_space { " " } else { "" };

        write!(w, "{}{}{}", typ, space, attrs)?;
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_ptr_helper(&self, attributes: Vec<PtrAttributes>, typ: TypeData) -> Result<String> {
        match typ {
            TypeData::MemberFunction(t) => self.dump_member_ptr(t, attributes),
            TypeData::Procedure(t) => self.dump_proc_ptr(t, attributes),
            _ => self.dump_other_ptr(typ, attributes),
        }
    }

    fn dump_ptr(&self, ptr: PointerType, is_const: bool) -> Result<String> {
        let mut attributes = Vec::new();
        attributes.push(PtrAttributes {
            is_pointer_const: ptr.attributes.is_const() || is_const,
            is_pointee_const: false,
            mode: ptr.attributes.pointer_mode(),
        });
        let mut ptr = ptr;
        loop {
            let typ = self.find(ptr.underlying_type)?;
            match typ {
                TypeData::Pointer(t) => {
                    attributes.push(PtrAttributes {
                        is_pointer_const: t.attributes.is_const(),
                        is_pointee_const: false,
                        mode: t.attributes.pointer_mode(),
                    });
                    ptr = t;
                }
                TypeData::Modifier(t) => {
                    // the vec cannot be empty since we push something in just before the loop
                    attributes.last_mut().unwrap().is_pointee_const = t.constant;
                    let typ = self.find(t.underlying_type)?;
                    if let TypeData::Pointer(t) = typ {
                        attributes.push(PtrAttributes {
                            is_pointer_const: t.attributes.is_const(),
                            is_pointee_const: false,
                            mode: t.attributes.pointer_mode(),
                        });
                        ptr = t;
                    } else {
                        return self.dump_ptr_helper(attributes, typ);
                    }
                }
                _ => {
                    return self.dump_ptr_helper(attributes, typ);
                }
            }
        }
    }

    fn get_array_info(&self, array: ArrayType) -> Result<(Vec<u32>, TypeData)> {
        // For an array int[12][34] it'll be represented as "int[34] *".
        // For any reason the 12 is lost...
        // The internal representation is: Pointer{ base: Array{ base: int, dim: 34 * sizeof(int)} }
        let mut base = array;
        let mut dims = Vec::new();
        dims.push(base.dimensions[0]);

        loop {
            let typ = self.find(base.element_type)?;
            match typ {
                TypeData::Array(a) => {
                    dims.push(a.dimensions[0]);
                    base = a;
                }
                _ => {
                    return Ok((dims, typ));
                }
            }
        }
    }

    fn dump_array(&self, array: ArrayType) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        let (dimensions, base) = self.get_array_info(array)?;
        let base_size = self.get_data_size(&base);
        let base_typ = self.dump_data(base)?;
        write!(w, "{}", base_typ)?;

        let mut size = base_size;
        let mut dims = dimensions
            .iter()
            .rev()
            .map(|dim| {
                let s = if size != 0 {
                    format!("[{}]", dim / size)
                } else {
                    // The base size can be zero: struct A{}; void foo(A x[10])
                    // No way to get the array dimension in such a case
                    "[]".to_string()
                };
                size = *dim;
                s
            })
            .collect::<Vec<String>>();
        dims.reverse();
        write!(w, "{}", dims.join(""))?;

        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_modifier(&self, modifier: ModifierType) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        let typ = self.find(modifier.underlying_type)?;
        match typ {
            TypeData::Pointer(ptr) => write!(w, "{}", self.dump_ptr(ptr, modifier.constant)?)?,
            TypeData::Primitive(prim) => {
                write!(w, "{}", self.dump_primitive(prim, modifier.constant)?)?
            }
            _ => {
                if modifier.constant {
                    write!(w, "const ")?
                }
                let underlying_typ = self.dump_data(typ)?;
                write!(w, "{}", underlying_typ)?
            }
        }
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_class(&self, class: ClassType) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        if self.flags.intersects(DumperFlags::NAME_ONLY) {
            write!(w, "{}", class.name)?;
        } else {
            let name = match class.kind {
                ClassKind::Class => "class",
                ClassKind::Interface => "interface",
                ClassKind::Struct => "struct",
            };
            write!(w, "{} {}", name, class.name)?
        }
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_arg_list(&self, list: ArgumentList) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        let comma = if self.flags.intersects(DumperFlags::SPACE_AFTER_COMMA) {
            ", "
        } else {
            ","
        };
        if let Some((last, args)) = list.arguments.split_last() {
            for index in args.iter() {
                let typ = self.dump_index(*index)?;
                write!(w, "{}", typ)?;
                write!(w, "{}", comma)?;
            }
            let typ = self.dump_index(*last)?;
            write!(w, "{}", typ)?;
        }
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_primitive(&self, prim: PrimitiveType, is_const: bool) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        // TODO: check that these names are what we want to see
        let name = match prim.kind {
            PrimitiveKind::NoType => "<NoType>",
            PrimitiveKind::Void => "void",
            PrimitiveKind::Char => "signed char",
            PrimitiveKind::UChar => "unsigned char",
            PrimitiveKind::RChar => "char",
            PrimitiveKind::WChar => "wchar_t",
            PrimitiveKind::RChar16 => "char16_t",
            PrimitiveKind::RChar32 => "char32_t",
            PrimitiveKind::I8 => "int8_t",
            PrimitiveKind::U8 => "uint8_t",
            PrimitiveKind::Short => "short",
            PrimitiveKind::UShort => "unsigned short",
            PrimitiveKind::I16 => "int16_t",
            PrimitiveKind::U16 => "uint16_t",
            PrimitiveKind::Long => "long",
            PrimitiveKind::ULong => "unsigned long",
            PrimitiveKind::I32 => "int",
            PrimitiveKind::U32 => "unsigned int",
            PrimitiveKind::Quad => "long long",
            PrimitiveKind::UQuad => "unsigned long long",
            PrimitiveKind::I64 => "int64_t",
            PrimitiveKind::U64 => "uint64_t",
            PrimitiveKind::I128 | PrimitiveKind::Octa => "int128_t",
            PrimitiveKind::U128 | PrimitiveKind::UOcta => "uint128_t",
            PrimitiveKind::F16 => "float16_t",
            PrimitiveKind::F32 => "float",
            PrimitiveKind::F32PP => "float",
            PrimitiveKind::F48 => "float48_t",
            PrimitiveKind::F64 => "double",
            PrimitiveKind::F80 => "long double",
            PrimitiveKind::F128 => "long double",
            PrimitiveKind::Complex32 => "complex<float>",
            PrimitiveKind::Complex64 => "complex<double>",
            PrimitiveKind::Complex80 => "complex<long double>",
            PrimitiveKind::Complex128 => "complex<long double>",
            PrimitiveKind::Bool8 => "bool",
            PrimitiveKind::Bool16 => "bool16_t",
            PrimitiveKind::Bool32 => "bool32_t",
            PrimitiveKind::Bool64 => "bool64_t",
            PrimitiveKind::HRESULT => "HRESULT",
        };

        if prim.indirection.is_some() {
            if self.flags.intersects(DumperFlags::SPACE_BEFORE_POINTER) {
                if is_const {
                    write!(w, "{} const *", name)?
                } else {
                    write!(w, "{} *", name)?
                }
            } else if is_const {
                write!(w, "{} const*", name)?
            } else {
                write!(w, "{}*", name)?
            }
        } else if is_const {
            write!(w, "const {}", name)?
        } else {
            write!(w, "{}", name)?
        }
        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_named(&self, base: &str, name: RawString) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        if self.flags.intersects(DumperFlags::NAME_ONLY) {
            write!(w, "{}", name)?
        } else {
            write!(w, "{} {}", base, name)?
        }

        Ok(String::from_utf8_lossy(&w).to_string())
    }

    fn dump_index(&self, index: TypeIndex) -> Result<String> {
        let typ = self.find(index)?;
        self.dump_data(typ)
    }

    fn dump_data(&self, typ: TypeData) -> Result<String> {
        let mut w: Vec<u8> = Vec::new();
        match typ {
            TypeData::Primitive(t) => write!(w, "{}", self.dump_primitive(t, false)?)?,
            TypeData::Class(t) => write!(w, "{}", self.dump_class(t)?)?,
            TypeData::MemberFunction(t) => {
                let ztatic = t.this_pointer_type.is_none();
                let no_return = self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN);
                let ret = self.get_return_type(Some(t.return_type), t.attributes, no_return);
                Self::dump_return(&mut w, ret)?;
                let (_, args) = self.dump_method_parts(t, ztatic)?;
                write!(w, "()({})", args)?
            }
            TypeData::Procedure(t) => {
                let no_return = self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN);
                let ret = self.get_return_type(t.return_type, t.attributes, no_return);
                Self::dump_return(&mut w, ret)?;
                let args = self.dump_index(t.argument_list)?;
                write!(w, "()({})", args)?;
            }
            TypeData::ArgumentList(t) => write!(w, "{}", self.dump_arg_list(t)?)?,
            TypeData::Pointer(t) => write!(w, "{}", self.dump_ptr(t, false)?)?,
            TypeData::Array(t) => write!(w, "{}", self.dump_array(t)?)?,
            TypeData::Union(t) => write!(w, "{}", self.dump_named("union", t.name)?)?,
            TypeData::Enumeration(t) => write!(w, "{}", self.dump_named("enum", t.name)?)?,
            TypeData::Enumerate(t) => write!(w, "{}", self.dump_named("enum class", t.name)?)?,
            TypeData::Modifier(t) => write!(w, "{}", self.dump_modifier(t)?)?,
            _ => write!(w, "unhandled type /* {:?} */", typ)?,
        }

        Ok(String::from_utf8_lossy(&w).to_string())
    }
}
