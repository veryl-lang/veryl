import "DPI-C" function chandle cosim_open       (input string path, input string top, input byte use_4state);
import "DPI-C" function void    cosim_close      (input chandle handle);
import "DPI-C" function void    cosim_step_reset (input chandle handle, input string name);
import "DPI-C" function void    cosim_step_clock (input chandle handle, input string name);
import "DPI-C" function void    cosim_set        (input chandle handle, input string name, input  logic [127:0] value);
import "DPI-C" function void    cosim_get        (input chandle handle, input string name, output logic [127:0] value);
