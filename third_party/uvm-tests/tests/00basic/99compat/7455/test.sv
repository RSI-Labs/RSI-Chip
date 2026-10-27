package test_pkg;

import uvm_pkg::*;
`include "uvm_macros.svh"

class test extends uvm_component;

`uvm_component_utils(test)

int check_count;
int error_count;

function new(string name = "test", uvm_component parent = null);
  super.new(name, parent);
  check_count = 0;
  error_count = 0;
endfunction

task run_phase(uvm_phase phase);
  `define to_string(x) `"x`"
  if(`to_string(`UVM_NAME) != "UVM") error_count++;
  if(`UVM_MAJOR_REV != 2020) error_count++;
  if(`UVM_MINOR_REV != 2.0) error_count++;
  if(`to_string(`UVM_VERSION_STRING) != "uvm_pkg::UVM_VERSION_STRING") error_count++;
  $display("define: %s variable %s", `to_string(`UVM_VERSION_STRING), uvm_pkg::UVM_VERSION_STRING);
  $display("define: %s variable %s", `to_string(`UVM_VERSION_STRING), uvm_revision_string());
  `ifdef UVM_POST_VERSION_1_1
    check_count++;
  `endif
  `ifdef UVM_POST_VERSION_1_2
    check_count++;
  `endif  
  `ifdef UVM_VERSION_POST_2017_1_0
    check_count++;
  `endif  
  `ifdef UVM_VERSION_POST_2017_1_1
    check_count++;
  `endif  
    `ifdef UVM_VERSION_POST_2020_1_0
    check_count++;
  `endif  
  `ifdef UVM_VERSION_POST_2020_1_1
    check_count++;
  `endif   
  `ifdef UVM_VERSION_POST_2017
    check_count++;
  `endif    
endtask

function void report_phase(uvm_phase phase);
  if((error_count == 0) && (check_count == 7)) begin
    `uvm_info("UVM TEST PASSED", "Back compat macro version ladder checks out OK", UVM_MEDIUM)
  end
  else begin
    `uvm_error("UVM TEST FAILED", "Error in back compat macro version ladder")
    $display("errors:%0d  checks:%0d", error_count, check_count);
  end
endfunction

endclass

endpackage

module test;

import uvm_pkg::*;
import test_pkg::*;

initial begin
  run_test("test");
end

endmodule  
