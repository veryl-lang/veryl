import random

import cocotb
from cocotb.clock import Clock
from cocotb.triggers import RisingEdge
from cocotb.types import LogicArray

@cocotb.test()
async def test(dut):
    #assert LogicArray(dut.o_d.value) == LogicArray("X")
    dut.i_d.value = 0
    clock = Clock(dut.i_clk, 10, units="us")
    cocotb.start_soon(clock.start(start_high=False))
    await RisingEdge(dut.i_clk)
    expected_val = 0
    for i in range(10):
        val = random.randint(0, 1)
        dut.i_d.value = val
        await RisingEdge(dut.i_clk)
        assert dut.o_d.value == expected_val, f"output q was incorrect on the {i}th cycle"
        expected_val = val
    await RisingEdge(dut.i_clk)
    assert dut.o_d.value == expected_val, "output q was incorrect on the last cycle"
