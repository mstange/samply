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
enum PtrToClassKind {
    PtrToGivenClass {
        /// If true, the pointer is a "pointer to const ClassType".
        constant: bool,
    },
    OtherType,
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
        const NO_MEMBER_FUNCTION_STATIC = 0b10;
        const SPACE_AFTER_COMMA = 0b100;
        const SPACE_BEFORE_POINTER = 0b1000;
        const NAME_ONLY = 0b10000;
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
                    _ => panic!("Unknown PrimitiveKind {:?} in get_data_size", t.kind),
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

    fn emit_parent_scope(&self, w: &mut impl Write, scope: ParentScope) -> Result<()> {
        match scope {
            ParentScope::WithType(scope_index) => match self.find(scope_index)? {
                TypeData::Class(c) => write!(w, "{}::", c.name)?,
                TypeData::Union(u) => write!(w, "{}::", u.name)?,
                TypeData::Enumeration(e) => write!(w, "{}::", e.name)?,
                other => write!(w, "<unhandled scope type {:?}>::", other)?,
            },
            ParentScope::WithId(id_data) => match id_data {
                pdb::IdData::String(s) => write!(w, "{}::", s.name)?,
                other => write!(w, "<unhandled id scope {:?}>::", other)?,
            },
        }
        Ok(())
    }

    /// Return a function or method signature, including return type (if requested),
    /// namespace and/or class qualifiers, and arguments.
    /// The function's name is really just the raw name. The arguments need to be
    /// obtained from its type information.
    /// If the TypeIndex is 0, then only the raw name is emitted. In that case, the
    /// name may need to go through additional demangling / "undecorating", but this
    /// is the responsibility of the caller.
    pub fn dump_function(
        &self,
        name: &str,
        function_type_index: TypeIndex,
        parent_scope: Option<ParentScope>,
    ) -> Result<String> {
        if function_type_index == TypeIndex(0) {
            if name.is_empty() {
                Ok("<name omitted>".to_string())
            } else {
                Ok(name.to_string())
            }
        } else {
            let mut w: Vec<u8> = Vec::new();
            let typ = self.find(function_type_index)?;
            match typ {
                TypeData::MemberFunction(t) => {
                    let is_static_method = t.this_pointer_type.is_none();
                    if is_static_method
                        && !self
                            .flags
                            .intersects(DumperFlags::NO_MEMBER_FUNCTION_STATIC)
                    {
                        w.write_all(b"static ")?;
                    }
                    if !self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN) {
                        self.emit_return_type(&mut w, Some(t.return_type), t.attributes)?;
                    }

                    if let Some(i) = parent_scope {
                        self.emit_parent_scope(&mut w, i)?;
                    }
                    if name.is_empty() {
                        write!(w, "<name omitted>")?;
                    } else {
                        write!(w, "{}", name)?;
                    };
                    let const_meth = self.emit_method_args(&mut w, t, is_static_method)?;
                    if const_meth {
                        w.write_all(b" const")?;
                    }
                }
                TypeData::Procedure(t) => {
                    if !self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN) {
                        self.emit_return_type(&mut w, t.return_type, t.attributes)?;
                    }

                    if let Some(i) = parent_scope {
                        self.emit_parent_scope(&mut w, i)?;
                    }
                    if name.is_empty() {
                        write!(w, "<name omitted>")?;
                    } else {
                        write!(w, "{}", name)?;
                    };
                    write!(w, "(")?;
                    self.emit_index(&mut w, t.argument_list)?;
                    write!(w, ")")?;
                }
                _ => return Ok(name.to_string()),
            }
            Ok(String::from_utf8(w)
                .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).to_string()))
        }
    }

    fn emit_return_type(
        &self,
        w: &mut impl Write,
        typ: Option<TypeIndex>,
        attrs: FunctionAttributes,
    ) -> Result<()> {
        if !attrs.is_constructor() {
            if let Some(index) = typ {
                self.emit_index(w, index)?;
                write!(w, " ")?;
            }
        }
        Ok(())
    }

    /// Check if ptr points to the specified class, and if so, whether it points to const or non-const class.
    /// If it points to a different class than the one supplied in the `class` argument, don't check constness.
    fn is_ptr_to_class(&self, ptr: TypeIndex, class: TypeIndex) -> Result<PtrToClassKind> {
        if let TypeData::Pointer(ptr_type) = self.find(ptr)? {
            let underlying_type = ptr_type.underlying_type;
            if underlying_type == class {
                return Ok(PtrToClassKind::PtrToGivenClass { constant: false });
            }
            let underlying_type_data = self.find(underlying_type)?;
            if let TypeData::Modifier(modifier) = underlying_type_data {
                if modifier.underlying_type == class {
                    return Ok(PtrToClassKind::PtrToGivenClass {
                        constant: modifier.constant,
                    });
                }
            }
        };
        Ok(PtrToClassKind::OtherType)
    }

    /// Return value: (this is pointer to const class, optional extra first argument)
    fn get_class_constness_and_extra_arguments(
        &self,
        this: TypeIndex,
        class: TypeIndex,
    ) -> Result<(bool, Option<TypeIndex>)> {
        match self.is_ptr_to_class(this, class)? {
            PtrToClassKind::PtrToGivenClass { constant } => {
                // The this type looks normal. Don't return an extra argument.
                Ok((constant, None))
            }
            PtrToClassKind::OtherType => {
                // The type of the "this" pointer did not match the class type.
                // This is arguably bad type information.
                // It looks like this bad type information is emitted for all Rust "associated
                // functions" whose first argument is a reference. Associated functions don't
                // take a self argument, so it would make sense to treat them as static.
                // But instead, these functions are marked as non-static, and the first argument's
                // type, rather than being part of the arguments list, is stored in the "this" type.
                // For example, for ProfileScope::new(name: &'static CStr), the arguments list is
                // empty and the this type is CStr*.
                // To work around this, return the this type as an extra first argument.
                Ok((false, Some(this)))
            }
        }
    }

    // Return value describes whether this is a const method.
    fn emit_method_args(
        &self,
        w: &mut impl Write,
        method_type: MemberFunctionType,
        is_static_method: bool,
    ) -> Result<bool> {
        let args_list = match self.find(method_type.argument_list)? {
            TypeData::ArgumentList(t) => t,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "argument type was not TypeData::ArgumentList",
                )
                .into());
            }
        };

        let (is_const_method, extra_first_arg) = if is_static_method {
            // Static methods have the correct arguments and are not const methods.
            (false, None)
        } else {
            // For non-static methods, check whether the method is const, and work around a
            // problem with bad type information for Rust associated functions.
            let this_type = method_type.this_pointer_type.unwrap();
            self.get_class_constness_and_extra_arguments(this_type, method_type.class_type)?
        };

        write!(w, "(")?;
        if let Some(first_arg) = extra_first_arg {
            self.emit_index(w, first_arg)?;
            self.emit_arg_list(w, args_list, true)?;
        } else {
            self.emit_arg_list(w, args_list, false)?;
        }
        write!(w, ")")?;

        Ok(is_const_method)
    }

    // Should we emit a space as the first byte from emit_attributes? It depends.
    // "*" in a table cell means "value has no impact on the outcome".
    //
    //  caller allows space | attributes start with | SPACE_BEFORE_POINTER mode | previous byte was   | put space at the beginning?
    // ---------------------+-----------------------+---------------------------+---------------------+----------------------------
    //  no                  | *                     | *                         | *                   | no
    //  yes                 | const                 | *                         | *                   | yes
    //  yes                 | pointer sigil         | off                       | *                   | no
    //  yes                 | pointer sigil         | on                        | pointer sigil       | no
    //  yes                 | pointer sigil         | on                        | not a pointer sigil | yes
    fn emit_attributes(
        &self,
        w: &mut impl Write,
        attrs: Vec<PtrAttributes>,
        allow_space_at_beginning: bool,
        mut previous_byte_was_pointer_sigil: bool,
    ) -> Result<()> {
        let mut is_at_beginning = true;
        for attr in attrs.iter().rev() {
            if attr.is_pointee_const {
                if !is_at_beginning || allow_space_at_beginning {
                    write!(w, " ")?;
                }
                write!(w, "const")?;
                is_at_beginning = false;
                previous_byte_was_pointer_sigil = false;
            }

            if self.flags.intersects(DumperFlags::SPACE_BEFORE_POINTER)
                && !previous_byte_was_pointer_sigil
            {
                if !is_at_beginning || allow_space_at_beginning {
                    write!(w, " ")?;
                }
            }
            is_at_beginning = false;
            match attr.mode {
                PointerMode::Pointer => write!(w, "*")?,
                PointerMode::LValueReference => write!(w, "&")?,
                PointerMode::Member => write!(w, "::*")?,
                PointerMode::MemberFunction => write!(w, "::*")?,
                PointerMode::RValueReference => write!(w, "&&")?,
            }
            previous_byte_was_pointer_sigil = true;
            if attr.is_pointer_const {
                write!(w, " const")?;
                previous_byte_was_pointer_sigil = false;
            }
        }
        Ok(())
    }

    fn emit_member_ptr(
        &self,
        w: &mut impl Write,
        fun: MemberFunctionType,
        attributes: Vec<PtrAttributes>,
    ) -> Result<()> {
        let ztatic = fun.this_pointer_type.is_none();
        self.emit_return_type(w, Some(fun.return_type), fun.attributes)?;

        write!(w, "(")?;
        self.emit_index(w, fun.class_type)?;
        self.emit_attributes(w, attributes, false, false)?;
        write!(w, ")")?;
        let _ = self.emit_method_args(w, fun, ztatic)?;
        Ok(())
    }

    fn emit_proc_ptr(
        &self,
        w: &mut impl Write,
        fun: ProcedureType,
        attributes: Vec<PtrAttributes>,
    ) -> Result<()> {
        self.emit_return_type(w, fun.return_type, fun.attributes)?;

        write!(w, "(")?;
        self.emit_attributes(w, attributes, false, false)?;
        write!(w, ")")?;
        write!(w, "(")?;
        self.emit_index(w, fun.argument_list)?;
        write!(w, ")")?;
        Ok(())
    }

    fn emit_other_ptr(
        &self,
        w: &mut impl Write,
        typ: TypeData,
        attributes: Vec<PtrAttributes>,
    ) -> Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        self.emit_data(&mut buf, typ)?;
        let previous_byte_was_pointer_sigil =
            buf.last().map(|&b| b == b'*' || b == b'&').unwrap_or(false);
        w.write_all(&buf)?;
        self.emit_attributes(w, attributes, true, previous_byte_was_pointer_sigil)?;

        Ok(())
    }

    fn emit_ptr_helper(
        &self,
        w: &mut impl Write,
        attributes: Vec<PtrAttributes>,
        typ: TypeData,
    ) -> Result<()> {
        match typ {
            TypeData::MemberFunction(t) => self.emit_member_ptr(w, t, attributes)?,
            TypeData::Procedure(t) => self.emit_proc_ptr(w, t, attributes)?,
            _ => self.emit_other_ptr(w, typ, attributes)?,
        };
        Ok(())
    }

    fn emit_ptr(&self, w: &mut impl Write, ptr: PointerType, is_const: bool) -> Result<()> {
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
                        self.emit_ptr_helper(w, attributes, typ)?;
                        return Ok(());
                    }
                }
                _ => {
                    self.emit_ptr_helper(w, attributes, typ)?;
                    return Ok(());
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

    fn emit_array(&self, w: &mut impl Write, array: ArrayType) -> Result<()> {
        let (dimensions_as_bytes, base) = self.get_array_info(array)?;
        let base_size = self.get_data_size(&base);
        self.emit_data(w, base)?;

        let mut iter = dimensions_as_bytes.into_iter().peekable();
        while let Some(current_level_byte_size) = iter.next() {
            let next_level_byte_size = *iter.peek().unwrap_or(&base_size);
            if next_level_byte_size != 0 {
                let element_count = current_level_byte_size / next_level_byte_size;
                write!(w, "[{}]", element_count)?;
            } else {
                // The base size can be zero: struct A{}; void foo(A x[10])
                // No way to get the array dimension in such a case
                write!(w, "[]")?;
            };
        }

        Ok(())
    }

    fn emit_modifier(&self, w: &mut impl Write, modifier: ModifierType) -> Result<()> {
        let typ = self.find(modifier.underlying_type)?;
        match typ {
            TypeData::Pointer(ptr) => self.emit_ptr(w, ptr, modifier.constant)?,
            TypeData::Primitive(prim) => self.emit_primitive(w, prim, modifier.constant)?,
            _ => {
                if modifier.constant {
                    write!(w, "const ")?
                }
                self.emit_data(w, typ)?;
            }
        }
        Ok(())
    }

    fn emit_class(&self, w: &mut impl Write, class: ClassType) -> Result<()> {
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
        Ok(())
    }

    fn emit_arg_list(
        &self,
        w: &mut impl Write,
        list: ArgumentList,
        comma_before_first: bool,
    ) -> Result<()> {
        if let Some((first, args)) = list.arguments.split_first() {
            if comma_before_first {
                write!(w, ",")?;
                if self.flags.intersects(DumperFlags::SPACE_AFTER_COMMA) {
                    write!(w, " ")?;
                }
            }
            self.emit_index(w, *first)?;
            for index in args.iter() {
                write!(w, ",")?;
                if self.flags.intersects(DumperFlags::SPACE_AFTER_COMMA) {
                    write!(w, " ")?;
                }
                self.emit_index(w, *index)?;
            }
        }
        Ok(())
    }

    fn emit_primitive(
        &self,
        w: &mut impl Write,
        prim: PrimitiveType,
        is_const: bool,
    ) -> Result<()> {
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
            _ => panic!("Unknown PrimitiveKind {:?} in emit_primitive", prim.kind),
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
        Ok(())
    }

    fn emit_named(&self, w: &mut impl Write, base: &str, name: RawString) -> Result<()> {
        if self.flags.intersects(DumperFlags::NAME_ONLY) {
            write!(w, "{}", name)?
        } else {
            write!(w, "{} {}", base, name)?
        }

        Ok(())
    }

    fn emit_index(&self, w: &mut impl Write, index: TypeIndex) -> Result<()> {
        let typ = self.find(index)?;
        self.emit_data(w, typ)?;
        Ok(())
    }

    fn emit_data(&self, w: &mut impl Write, typ: TypeData) -> Result<()> {
        match typ {
            TypeData::Primitive(t) => self.emit_primitive(w, t, false)?,
            TypeData::Class(t) => self.emit_class(w, t)?,
            TypeData::MemberFunction(t) => {
                let ztatic = t.this_pointer_type.is_none();
                if !self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN) {
                    self.emit_return_type(w, Some(t.return_type), t.attributes)?;
                }

                write!(w, "()")?;
                let _ = self.emit_method_args(w, t, ztatic)?;
            }
            TypeData::Procedure(t) => {
                if !self.flags.intersects(DumperFlags::NO_FUNCTION_RETURN) {
                    self.emit_return_type(w, t.return_type, t.attributes)?;
                }

                write!(w, "()(")?;
                self.emit_index(w, t.argument_list)?;
                write!(w, "")?;
            }
            TypeData::ArgumentList(t) => self.emit_arg_list(w, t, false)?,
            TypeData::Pointer(t) => self.emit_ptr(w, t, false)?,
            TypeData::Array(t) => self.emit_array(w, t)?,
            TypeData::Union(t) => self.emit_named(w, "union", t.name)?,
            TypeData::Enumeration(t) => self.emit_named(w, "enum", t.name)?,
            TypeData::Enumerate(t) => self.emit_named(w, "enum class", t.name)?,
            TypeData::Modifier(t) => self.emit_modifier(w, t)?,
            _ => write!(w, "unhandled type /* {:?} */", typ)?,
        }

        Ok(())
    }
}
