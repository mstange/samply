/*
 * This looks dumb on purpose.
 *
 * The bug in #776 is a layout bug: we need a non-PIE ELF with a non-zero base
 * address and enough function/FDE entries that the old synthetic `fun_*`
 * symbols end up landing inside the range of some real function.
 *
 * Perhaps this is not the best way to trigger this, but it is the one I could
 * think of: use a pile of `noinline` functions with padded NOP bodies to force
 * that layout reliably.
 */
#define ATTR __attribute__((noinline,used))
ATTR int func_0(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 0; }
ATTR int func_1(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 1; }
ATTR int func_2(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 2; }
ATTR int func_3(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 3; }
ATTR int func_4(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 4; }
ATTR int func_5(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 5; }
ATTR int func_6(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 6; }
ATTR int func_7(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 7; }
ATTR int func_8(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 8; }
ATTR int func_9(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 9; }
ATTR int func_10(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 10; }
ATTR int func_11(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 11; }
ATTR int func_12(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 12; }
ATTR int func_13(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 13; }
ATTR int func_14(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 14; }
ATTR int func_15(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 15; }
ATTR int func_16(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 16; }
ATTR int func_17(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 17; }
ATTR int func_18(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 18; }
ATTR int func_19(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 19; }
ATTR int func_20(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 20; }
ATTR int func_21(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 21; }
ATTR int func_22(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 22; }
ATTR int func_23(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 23; }
ATTR int func_24(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 24; }
ATTR int func_25(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 25; }
ATTR int func_26(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 26; }
ATTR int func_27(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 27; }
ATTR int func_28(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 28; }
ATTR int func_29(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 29; }
ATTR int func_30(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 30; }
ATTR int func_31(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 31; }
ATTR int func_32(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 32; }
ATTR int func_33(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 33; }
ATTR int func_34(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 34; }
ATTR int func_35(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 35; }
ATTR int func_36(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 36; }
ATTR int func_37(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 37; }
ATTR int func_38(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 38; }
ATTR int func_39(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 39; }
ATTR int func_40(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 40; }
ATTR int func_41(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 41; }
ATTR int func_42(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 42; }
ATTR int func_43(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 43; }
ATTR int func_44(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 44; }
ATTR int func_45(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 45; }
ATTR int func_46(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 46; }
ATTR int func_47(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 47; }
ATTR int func_48(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 48; }
ATTR int func_49(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 49; }
ATTR int func_50(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 50; }
ATTR int func_51(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 51; }
ATTR int func_52(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 52; }
ATTR int func_53(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 53; }
ATTR int func_54(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 54; }
ATTR int func_55(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 55; }
ATTR int func_56(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 56; }
ATTR int func_57(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 57; }
ATTR int func_58(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 58; }
ATTR int func_59(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 59; }
ATTR int func_60(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 60; }
ATTR int func_61(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 61; }
ATTR int func_62(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 62; }
ATTR int func_63(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 63; }
ATTR int func_64(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 64; }
ATTR int func_65(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 65; }
ATTR int func_66(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 66; }
ATTR int func_67(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 67; }
ATTR int func_68(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 68; }
ATTR int func_69(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 69; }
ATTR int func_70(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 70; }
ATTR int func_71(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 71; }
ATTR int func_72(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 72; }
ATTR int func_73(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 73; }
ATTR int func_74(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 74; }
ATTR int func_75(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 75; }
ATTR int func_76(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 76; }
ATTR int func_77(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 77; }
ATTR int func_78(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 78; }
ATTR int func_79(int x) { asm volatile(".rept 48\n\tnop\n\t.endr" ::: "memory"); return x + 79; }
int main(void) { volatile int sum = 0; sum += func_0(1); sum += func_79(2); return sum; }
