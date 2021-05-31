use bitflags::bitflags;
use pdb::{
    ArgumentList, ArrayType, ClassKind, ClassType, DebugInformation, FallibleIterator,
    FunctionAttributes, IdData, IdFinder, IdIndex, IdInformation, MachineType, MemberFunctionType,
    ModifierType, PointerMode, PointerType, PrimitiveKind, PrimitiveType, ProcedureType, RawString,
    TypeData, TypeFinder, TypeIndex, TypeInformation, UnionType, Variant,
};
use std::collections::HashMap;
use std::fmt::Write;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Formatting error: {0}")]
    FormatError(#[source] std::fmt::Error),

    #[error("PDB error: {0}")]
    PdbError(#[source] pdb::Error),

    #[error("Unexpected type for argument list")]
    ArgumentTypeNotArgumentList,

    #[error("Id of type Function doesn't have type of Procedure")]
    FunctionIdIsNotProcedureType,

    #[error("Id of type MemberFunction doesn't have type of MemberFunction")]
    MemberFunctionIdIsNotMemberFunctionType,
}

impl From<pdb::Error> for Error {
    fn from(err: pdb::Error) -> Self {
        Self::PdbError(err)
    }
}

impl From<std::fmt::Error> for Error {
    fn from(err: std::fmt::Error) -> Self {
        Self::FormatError(err)
    }
}

type Result<V> = std::result::Result<V, Error>;

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
    pub struct TypeFormatterFlags: u32 {
        const NO_FUNCTION_RETURN = 0b1;
        const NO_MEMBER_FUNCTION_STATIC = 0b10;
        const SPACE_AFTER_COMMA = 0b100;
        const SPACE_BEFORE_POINTER = 0b1000;
        const NAME_ONLY = 0b10000;
    }
}

impl Default for TypeFormatterFlags {
    fn default() -> Self {
        Self::NO_FUNCTION_RETURN | Self::SPACE_AFTER_COMMA | Self::NAME_ONLY
    }
}

pub struct TypeFormatter<'t> {
    type_finder: TypeFinder<'t>,
    id_finder: IdFinder<'t>,

    /// A hashmap that maps a type's (unique) name to its type size.
    forward_ref_sizes: HashMap<RawString<'t>, u32>,

    ptr_size: u32,
    flags: TypeFormatterFlags,
}

impl<'t> TypeFormatter<'t> {
    /// Collect all the Type and their TypeIndex to be able to search for a TypeIndex
    pub fn new(
        debug_info: &'t DebugInformation<'_>,
        type_info: &'t TypeInformation<'_>,
        id_info: &'t IdInformation<'_>,
        flags: TypeFormatterFlags,
    ) -> std::result::Result<Self, pdb::Error> {
        let mut type_iter = type_info.iter();
        let mut type_finder = type_info.finder();

        // When computing type sizes, special care must be taken for types which are
        // marked as "forward references": For these types, the size must be taken from
        // the occurrence of the type with the same (unique) name which is not marked as
        // a forward reference.
        // In order to be able to look up these sizes, we create a map upfront, which
        // contains all sizes  for non-forward_reference types.
        // Type sizes are needed when computing array lengths based on byte lengths, when
        // printing array types. They are also needed for the public get_type_size method.
        let mut forward_ref_sizes = HashMap::new();

        while let Some(item) = type_iter.next()? {
            type_finder.update(&type_iter);
            if let Ok(type_data) = item.parse() {
                match type_data {
                    TypeData::Class(t) => {
                        if !t.properties.forward_reference() {
                            let name = t.unique_name.unwrap_or(t.name);
                            forward_ref_sizes.insert(name, t.size.into());
                        }
                    }
                    TypeData::Union(t) => {
                        if !t.properties.forward_reference() {
                            let name = t.unique_name.unwrap_or(t.name);
                            forward_ref_sizes.insert(name, t.size);
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut id_finder = id_info.finder();
        let mut id_iter = id_info.iter();
        while let Some(_) = id_iter.next()? {
            id_finder.update(&id_iter);
        }

        let ptr_size = match debug_info.machine_type()? {
            MachineType::Amd64 | MachineType::Arm64 | MachineType::Ia64 | MachineType::RiscV64 => 8,
            MachineType::RiscV128 => 16,
            _ => 4,
        };

        Ok(Self {
            type_finder,
            id_finder,
            forward_ref_sizes,
            ptr_size,
            flags,
        })
    }

    pub fn get_type_size(&self, index: TypeIndex) -> u32 {
        if let Ok(type_data) = self.resolve_type_index(index) {
            self.get_data_size(&type_data)
        } else {
            0
        }
    }

    /// Write out the function or method signature, including return type (if requested),
    /// namespace and/or class qualifiers, and arguments.
    /// The function's name is really just the raw name. The arguments need to be
    /// obtained from its type information.
    /// If the TypeIndex is 0, then only the raw name is emitted. In that case, the
    /// name may need to go through additional demangling / "undecorating", but this
    /// is the responsibility of the caller.
    pub fn write_function(
        &self,
        w: &mut impl Write,
        name: &str,
        function_type_index: TypeIndex,
    ) -> Result<()> {
        if function_type_index == TypeIndex(0) {
            return self.emit_name_str(w, name);
        }

        match self.resolve_type_index(function_type_index)? {
            TypeData::MemberFunction(t) => {
                if t.this_pointer_type.is_none() {
                    self.maybe_emit_static(w)?;
                }
                self.maybe_emit_return_type(w, Some(t.return_type), t.attributes)?;
                self.emit_name_str(w, name)?;
                self.emit_method_args(w, t, true)?;
            }
            TypeData::Procedure(t) => {
                self.maybe_emit_return_type(w, t.return_type, t.attributes)?;
                self.emit_name_str(w, name)?;
                write!(w, "(")?;
                self.emit_type_index(w, t.argument_list)?;
                write!(w, ")")?;
            }
            _ => {
                write!(w, "{}", name)?;
            }
        }
        Ok(())
    }

    pub fn write_id(&self, w: &mut impl Write, id_index: IdIndex) -> Result<()> {
        match self.resolve_id_index(id_index)? {
            IdData::MemberFunction(m) => {
                let t = match self.resolve_type_index(m.function_type)? {
                    TypeData::MemberFunction(t) => t,
                    _ => return Err(Error::MemberFunctionIdIsNotMemberFunctionType),
                };

                let is_static_method = t.this_pointer_type.is_none();
                if is_static_method {
                    self.maybe_emit_static(w)?;
                }
                self.maybe_emit_return_type(w, Some(t.return_type), t.attributes)?;
                self.emit_type_index(w, m.parent)?;
                write!(w, "::")?;
                self.emit_name_str(w, &m.name.to_string())?;
                self.emit_method_args(w, t, true)?;
            }
            IdData::Function(f) => {
                let t = match self.resolve_type_index(f.function_type)? {
                    TypeData::Procedure(t) => t,
                    _ => return Err(Error::FunctionIdIsNotProcedureType),
                };

                self.maybe_emit_return_type(w, t.return_type, t.attributes)?;
                if let Some(scope) = f.scope {
                    self.write_id(w, scope)?;
                    write!(w, "::")?;
                }
                self.emit_name_str(w, &f.name.to_string())?;
                write!(w, "(")?;
                self.emit_type_index(w, t.argument_list)?;
                write!(w, ")")?;
            }
            IdData::String(s) => write!(w, "{}", s.name)?,
            other => write!(w, "<unhandled id scope {:?}>::", other)?,
        }
        Ok(())
    }

    fn resolve_type_index(&self, index: TypeIndex) -> Result<TypeData> {
        let item = self.type_finder.find(index).unwrap();
        Ok(item.parse()?)
    }

    fn resolve_id_index(&self, index: IdIndex) -> Result<IdData> {
        let item = self.id_finder.find(index).unwrap();
        Ok(item.parse()?)
    }

    fn get_class_size(&self, class_type: &ClassType) -> u32 {
        if class_type.properties.forward_reference() {
            let name = class_type.unique_name.unwrap_or(class_type.name);

            // Sometimes the name will not be in self.forward_ref_sizes - this can occur for
            // the empty struct, which can be a forward reference to itself!
            *self
                .forward_ref_sizes
                .get(&name)
                .unwrap_or(&class_type.size.into())
        } else {
            class_type.size.into()
        }
    }

    fn get_union_size(&self, union_type: &UnionType) -> u32 {
        if union_type.properties.forward_reference() {
            let name = union_type.unique_name.unwrap_or(union_type.name);
            *self
                .forward_ref_sizes
                .get(&name)
                .unwrap_or(&union_type.size)
        } else {
            union_type.size
        }
    }

    fn get_data_size(&self, type_data: &TypeData) -> u32 {
        match type_data {
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

    fn has_flags(&self, flags: TypeFormatterFlags) -> bool {
        self.flags.intersects(flags)
    }

    fn maybe_emit_static(&self, w: &mut impl Write) -> Result<()> {
        if self.has_flags(TypeFormatterFlags::NO_MEMBER_FUNCTION_STATIC) {
            return Ok(());
        }

        w.write_str("static ")?;
        Ok(())
    }

    fn maybe_emit_return_type(
        &self,
        w: &mut impl Write,
        type_index: Option<TypeIndex>,
        attrs: FunctionAttributes,
    ) -> Result<()> {
        if self.has_flags(TypeFormatterFlags::NO_FUNCTION_RETURN) {
            return Ok(());
        }

        self.emit_return_type(w, type_index, attrs)?;
        Ok(())
    }

    fn emit_name_str(&self, w: &mut impl Write, name: &str) -> Result<()> {
        if name.is_empty() {
            write!(w, "<name omitted>")?;
        } else {
            write!(w, "{}", name)?;
        }
        Ok(())
    }

    fn emit_return_type(
        &self,
        w: &mut impl Write,
        type_index: Option<TypeIndex>,
        attrs: FunctionAttributes,
    ) -> Result<()> {
        if !attrs.is_constructor() {
            if let Some(index) = type_index {
                self.emit_type_index(w, index)?;
                write!(w, " ")?;
            }
        }
        Ok(())
    }

    /// Check if ptr points to the specified class, and if so, whether it points to const or non-const class.
    /// If it points to a different class than the one supplied in the `class` argument, don't check constness.
    fn is_ptr_to_class(&self, ptr: TypeIndex, class: TypeIndex) -> Result<PtrToClassKind> {
        if let TypeData::Pointer(ptr_type) = self.resolve_type_index(ptr)? {
            let underlying_type = ptr_type.underlying_type;
            if underlying_type == class {
                return Ok(PtrToClassKind::PtrToGivenClass { constant: false });
            }
            let underlying_type_data = self.resolve_type_index(underlying_type)?;
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

    fn emit_method_args(
        &self,
        w: &mut impl Write,
        method_type: MemberFunctionType,
        allow_emit_const: bool,
    ) -> Result<()> {
        let args_list = match self.resolve_type_index(method_type.argument_list)? {
            TypeData::ArgumentList(t) => t,
            _ => {
                return Err(Error::ArgumentTypeNotArgumentList);
            }
        };

        let (is_const_method, extra_first_arg) = match method_type.this_pointer_type {
            None => {
                // No this pointer - this is a static method.
                // Static methods cannot be const, and they have the correct arguments.
                (false, None)
            }
            Some(this_type) => {
                // For non-static methods, check whether the method is const, and work around a
                // problem with bad type information for Rust associated functions.
                self.get_class_constness_and_extra_arguments(this_type, method_type.class_type)?
            }
        };

        write!(w, "(")?;
        if let Some(first_arg) = extra_first_arg {
            self.emit_type_index(w, first_arg)?;
            self.emit_arg_list(w, args_list, true)?;
        } else {
            self.emit_arg_list(w, args_list, false)?;
        }
        write!(w, ")")?;

        if is_const_method && allow_emit_const {
            write!(w, " const")?;
        }

        Ok(())
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

            if self.has_flags(TypeFormatterFlags::SPACE_BEFORE_POINTER)
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
        self.emit_return_type(w, Some(fun.return_type), fun.attributes)?;
        write!(w, "(")?;
        self.emit_type_index(w, fun.class_type)?;
        self.emit_attributes(w, attributes, false, false)?;
        write!(w, ")")?;
        self.emit_method_args(w, fun, false)?;
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
        self.emit_type_index(w, fun.argument_list)?;
        write!(w, ")")?;
        Ok(())
    }

    fn emit_other_ptr(
        &self,
        w: &mut impl Write,
        type_data: TypeData,
        attributes: Vec<PtrAttributes>,
    ) -> Result<()> {
        let mut buf = String::new();
        self.emit_type(&mut buf, type_data)?;
        let previous_byte_was_pointer_sigil = buf
            .as_bytes()
            .last()
            .map(|&b| b == b'*' || b == b'&')
            .unwrap_or(false);
        w.write_str(&buf)?;
        self.emit_attributes(w, attributes, true, previous_byte_was_pointer_sigil)?;

        Ok(())
    }

    fn emit_ptr_helper(
        &self,
        w: &mut impl Write,
        attributes: Vec<PtrAttributes>,
        type_data: TypeData,
    ) -> Result<()> {
        match type_data {
            TypeData::MemberFunction(t) => self.emit_member_ptr(w, t, attributes)?,
            TypeData::Procedure(t) => self.emit_proc_ptr(w, t, attributes)?,
            _ => self.emit_other_ptr(w, type_data, attributes)?,
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
            let type_data = self.resolve_type_index(ptr.underlying_type)?;
            match type_data {
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
                    let underlying_type_data = self.resolve_type_index(t.underlying_type)?;
                    if let TypeData::Pointer(t) = underlying_type_data {
                        attributes.push(PtrAttributes {
                            is_pointer_const: t.attributes.is_const(),
                            is_pointee_const: false,
                            mode: t.attributes.pointer_mode(),
                        });
                        ptr = t;
                    } else {
                        self.emit_ptr_helper(w, attributes, underlying_type_data)?;
                        return Ok(());
                    }
                }
                _ => {
                    self.emit_ptr_helper(w, attributes, type_data)?;
                    return Ok(());
                }
            }
        }
    }

    /// The returned Vec has the array dimensions in bytes, with the "lower" dimensions
    /// aggregated into the "higher" dimensions.
    fn get_array_info(&self, array: ArrayType) -> Result<(Vec<u32>, TypeData)> {
        // For an array int[12][34] it'll be represented as "int[34] *".
        // For any reason the 12 is lost...
        // The internal representation is: Pointer{ base: Array{ base: int, dim: 34 * sizeof(int)} }
        let mut base = array;
        let mut dims = Vec::new();
        dims.push(base.dimensions[0]);

        // See the documentation for ArrayType::dimensions:
        //
        // > Contains array dimensions as specified in the PDB. This is not what you expect:
        // >
        // > * Dimensions are specified in terms of byte sizes, not element counts.
        // > * Multidimensional arrays aggregate the lower dimensions into the sizes of the higher
        // >   dimensions.
        // >
        // > Thus a `float[4][4]` has `dimensions: [16, 64]`. Determining array dimensions in terms
        // > of element counts requires determining the size of the `element_type` and iteratively
        // > dividing.
        //
        // XXXmstange the docs above imply that dimensions can have more than just one entry.
        // But this code only processes dimensions[0]. Is that a bug?
        loop {
            let type_data = self.resolve_type_index(base.element_type)?;
            match type_data {
                TypeData::Array(a) => {
                    dims.push(a.dimensions[0]);
                    base = a;
                }
                _ => {
                    return Ok((dims, type_data));
                }
            }
        }
    }

    fn emit_array(&self, w: &mut impl Write, array: ArrayType) -> Result<()> {
        let (dimensions_as_bytes, base) = self.get_array_info(array)?;
        let base_size = self.get_data_size(&base);
        self.emit_type(w, base)?;

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
        let type_data = self.resolve_type_index(modifier.underlying_type)?;
        match type_data {
            TypeData::Pointer(ptr) => self.emit_ptr(w, ptr, modifier.constant)?,
            TypeData::Primitive(prim) => self.emit_primitive(w, prim, modifier.constant)?,
            _ => {
                if modifier.constant {
                    write!(w, "const ")?
                }
                self.emit_type(w, type_data)?;
            }
        }
        Ok(())
    }

    fn emit_class(&self, w: &mut impl Write, class: ClassType) -> Result<()> {
        if self.has_flags(TypeFormatterFlags::NAME_ONLY) {
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
                if self.has_flags(TypeFormatterFlags::SPACE_AFTER_COMMA) {
                    write!(w, " ")?;
                }
            }
            self.emit_type_index(w, *first)?;
            for index in args.iter() {
                write!(w, ",")?;
                if self.has_flags(TypeFormatterFlags::SPACE_AFTER_COMMA) {
                    write!(w, " ")?;
                }
                self.emit_type_index(w, *index)?;
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
            if self.has_flags(TypeFormatterFlags::SPACE_BEFORE_POINTER) {
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
        if self.has_flags(TypeFormatterFlags::NAME_ONLY) {
            write!(w, "{}", name)?
        } else {
            write!(w, "{} {}", base, name)?
        }

        Ok(())
    }

    fn emit_type_index(&self, w: &mut impl Write, index: TypeIndex) -> Result<()> {
        self.emit_type(w, self.resolve_type_index(index)?)
    }

    fn emit_type(&self, w: &mut impl Write, type_data: TypeData) -> Result<()> {
        match type_data {
            TypeData::Primitive(t) => self.emit_primitive(w, t, false)?,
            TypeData::Class(t) => self.emit_class(w, t)?,
            TypeData::MemberFunction(t) => {
                self.maybe_emit_return_type(w, Some(t.return_type), t.attributes)?;
                write!(w, "()")?;
                self.emit_method_args(w, t, false)?;
            }
            TypeData::Procedure(t) => {
                self.maybe_emit_return_type(w, t.return_type, t.attributes)?;
                write!(w, "()(")?;
                self.emit_type_index(w, t.argument_list)?;
                write!(w, "")?;
            }
            TypeData::ArgumentList(t) => self.emit_arg_list(w, t, false)?,
            TypeData::Pointer(t) => self.emit_ptr(w, t, false)?,
            TypeData::Array(t) => self.emit_array(w, t)?,
            TypeData::Union(t) => self.emit_named(w, "union", t.name)?,
            TypeData::Enumeration(t) => self.emit_named(w, "enum", t.name)?,
            TypeData::Enumerate(t) => self.emit_named(w, "enum class", t.name)?,
            TypeData::Modifier(t) => self.emit_modifier(w, t)?,
            _ => write!(w, "unhandled type /* {:?} */", type_data)?,
        }

        Ok(())
    }
}
